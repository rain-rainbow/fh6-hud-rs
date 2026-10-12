//! `fh6-relay` 的端到端集成测试。
//!
//! 这些测试用**真实 UDP 套接字**验证转发链，而不是 mock：
//!
//! ```text
//!   发送端 ──► relay 监听端口 ──► 订阅者 A
//!                            └─► 订阅者 B
//! ```
//!
//! 之所以能做到「不启动子进程、不连着游戏」，是因为转发内核被拆到了
//! `fh6_relay::Relay` 这个可测试结构体里。
//!
//! # 测试基础设施为什么要写得这么小心
//!
//! 本测试在开发过程中曾出现「5 个用例一起失败、但手工诊断却完全正常」的情况。
//! 根因不是产品代码，而是**测试脚手架**：
//!
//! 1. 先 `bind(0)` 拿端口号、再 `drop` 让 relay 去绑同一个号 —— 中间存在
//!    竞态窗口，端口可能被别人抢走，导致 relay 绑不上；
//! 2. relay 在**独立线程**里，绑定失败只是让线程 panic，测试侧只看到
//!    「订阅者收不到包」，极易误判为转发逻辑有 bug。
//!
//! 因此这里改为：
//! * 由**主线程**绑定监听套接字，再把它**直接交接**给 relay 线程 ——
//!   端口从绑定到使用之间从未进入空闲状态，竞态窗口被彻底消除；
//! * relay 线程内接管失败**直接 panic 并带上错误信息**，让失败可读。
//!
//! 测试函数名用中文描述意图，因此关闭 `non_snake_case` 检查。
#![allow(non_snake_case)]

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use fh6_core::Telemetry;
use fh6_relay::{parse_host_port, Rate, Relay, Stats};

/// 绑定一个临时端口（端口传 0 由系统分配），返回套接字与端口。
fn bind_ephemeral() -> (UdpSocket, u16) {
    let s = UdpSocket::bind("127.0.0.1:0").expect("绑定临时端口失败");
    let port = s.local_addr().unwrap().port();
    (s, port)
}

/// 一个「持有中」的端口。
///
/// 字段 `_socket` **故意从不读取**：它的唯一作用就是通过持有它来占住端口，
/// 让「端口已被占用」的用例能够稳定复现。变量名前缀下划线是为了向
/// 阅读者和 lint 表明「这不是死代码，是刻意的 RAII 占位」。
struct ReservedPort {
    _socket: UdpSocket,
    port: u16,
}

impl ReservedPort {
    fn new() -> Self {
        let (socket, port) = bind_ephemeral();
        Self {
            _socket: socket,
            port,
        }
    }

    /// 只取端口号，不释放。
    fn port(&self) -> u16 {
        self.port
    }
}

/// 在 `127.0.0.1:0` 上绑定，返回 (套接字, 端口)。
///
/// 套接字**保持绑定状态**并交给 relay 使用，从而完全避免
/// 「释放端口再重绑」的竞态窗口。
fn bind_relay_socket() -> (UdpSocket, u16) {
    bind_ephemeral()
}

/// 构造一个可识别的测试数据包（324 字节，内容由 seed 决定）。
fn make_packet(seed: u8) -> [u8; fh6_core::PACKET_SIZE] {
    let t = Telemetry {
        is_race_on: true,
        timestamp_ms: 1_000_000 + seed as u32,
        speed_mps: 10.0 + seed as f32,
        velocity: [seed as f32, 0.0, 20.0],
        current_engine_rpm: 5000.0 + seed as f32,
        gear_raw: seed % 8,
        ..Default::default()
    };
    t.encode()
}

/// 带超时地等待一个 UDP 包，返回字节内容。
fn recv_with_timeout(sock: &UdpSocket, timeout: Duration) -> Option<Vec<u8>> {
    sock.set_read_timeout(Some(timeout)).ok()?;
    let mut buf = [0u8; 2048];
    match sock.recv_from(&mut buf) {
        Ok((len, _)) => Some(buf[..len].to_vec()),
        Err(_) => None,
    }
}

/// relay 线程的句柄。
struct RelayHandle {
    stop: Arc<AtomicBool>,
    join: thread::JoinHandle<()>,
}

impl RelayHandle {
    fn stop_and_join(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.join.join();
    }
}

/// 启动 relay 线程，**接管调用方已绑定的套接字**。
///
/// 套接字由主线程绑定后直接交接给 relay 线程，端口从未进入空闲状态，
/// 因此不存在「被别的测试抢走」的可能。
fn spawn_relay(socket: UdpSocket, targets: Vec<(String, u16)>) -> RelayHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);

    let join = thread::spawn(move || {
        // 接管失败就 panic —— 在测试里这是正确行为，
        // 因为它意味着测试环境有问题，而不是"包丢了"。
        let mut relay = match Relay::from_socket(socket, &targets) {
            Ok(r) => r,
            Err(e) => panic!("relay 接管套接字失败: {e}"),
        };
        relay
            .set_read_timeout(Some(Duration::from_millis(20)))
            .expect("设置读取超时失败");
        let _ = relay.run(move |_stats| stop2.load(Ordering::Relaxed));
    });

    RelayHandle { stop, join }
}

/// 一步到位：绑定套接字、启动 relay。返回 (relay 句柄, 监听端口)。
///
/// relay 启动时即已持有套接字，**不需要等待"绑定成功"**（端口从未空闲）。
/// 只做一个短暂等待，确保收包循环已经开始，避免发送端先发包导致丢包。
fn start_relay(targets: Vec<(String, u16)>) -> (RelayHandle, u16) {
    let (socket, port) = bind_relay_socket();
    let handle = spawn_relay(socket, targets);
    thread::sleep(Duration::from_millis(50));
    (handle, port)
}

#[test]
fn 一个数据包应被转发给全部订阅者() {
    let (sub_a, port_a) = bind_ephemeral();
    let (sub_b, port_b) = bind_ephemeral();

    let (relay, relay_port) = start_relay(vec![
        ("127.0.0.1".to_string(), port_a),
        ("127.0.0.1".to_string(), port_b),
    ]);

    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let packet = make_packet(7);
    sender
        .send_to(&packet, format!("127.0.0.1:{relay_port}"))
        .expect("发送失败");

    let got_a = recv_with_timeout(&sub_a, Duration::from_secs(2));
    let got_b = recv_with_timeout(&sub_b, Duration::from_secs(2));

    relay.stop_and_join();

    assert_eq!(
        got_a.as_deref(),
        Some(packet.as_slice()),
        "订阅者 A 未收到原样字节"
    );
    assert_eq!(
        got_b.as_deref(),
        Some(packet.as_slice()),
        "订阅者 B 未收到原样字节"
    );
}

#[test]
fn 转发必须逐字节透明() {
    // 这是最关键的性质：飞智的扳机模块直接吃原始字节，
    // relay 只要改动任何一个字节（哪怕补个包头）都会让它解析失败。
    let (sub, sub_port) = bind_ephemeral();
    let (relay, relay_port) = start_relay(vec![("127.0.0.1".to_string(), sub_port)]);
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();

    // 用「全字节遍历」的载荷，确保任何一个字节位置都不能出错
    let mut packet = [0u8; fh6_core::PACKET_SIZE];
    for (i, b) in packet.iter_mut().enumerate() {
        *b = (i % 251) as u8; // 251 是质数，避免周期性对齐掩盖错误
    }
    sender
        .send_to(&packet, format!("127.0.0.1:{relay_port}"))
        .unwrap();

    let got = recv_with_timeout(&sub, Duration::from_secs(2));
    relay.stop_and_join();

    let got = got.expect("未收到数据包");
    assert_eq!(got.len(), fh6_core::PACKET_SIZE, "包长必须保持不变");
    assert_eq!(got, packet.to_vec(), "字节内容必须完全一致");
}

#[test]
fn 连续多个数据包应全部转发() {
    let (sub, sub_port) = bind_ephemeral();
    let (relay, relay_port) = start_relay(vec![("127.0.0.1".to_string(), sub_port)]);
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();

    const N: u8 = 20;
    for i in 0..N {
        sender
            .send_to(&make_packet(i), format!("127.0.0.1:{relay_port}"))
            .unwrap();
    }

    // 收齐 N 个包
    sub.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut received = Vec::new();
    let mut buf = [0u8; 2048];
    let deadline = Instant::now() + Duration::from_secs(5);
    while received.len() < N as usize && Instant::now() < deadline {
        match sub.recv_from(&mut buf) {
            Ok((len, _)) => received.push(buf[..len].to_vec()),
            Err(_) => break,
        }
    }

    relay.stop_and_join();

    assert_eq!(received.len(), N as usize, "应当收到全部 {N} 个包");
    // UDP 在回环上不保证顺序，但内容集合必须完整
    for i in 0..N {
        let expect = make_packet(i);
        assert!(
            received.iter().any(|p| p.as_slice() == expect.as_slice()),
            "缺少第 {i} 个包"
        );
    }
}

#[test]
fn 目标端口无人监听时不应崩溃且继续转发给其他目标() {
    // 找一个确实无人监听的端口。做法：绑定后立刻释放并**立即**用作目标 ——
    // 这里不怕竞态，因为最坏情况只是"该端口恰好又被人监听"，
    // 而 relay 的容错行为（不退出）与目标是否可达无关。
    let dead_port = {
        let (s, p) = bind_ephemeral();
        drop(s);
        p
    };

    let (alive, alive_port) = bind_ephemeral();
    // 第一个目标指向没人监听的端口，第二个才是活的
    let (relay, relay_port) = start_relay(vec![
        ("127.0.0.1".to_string(), dead_port),
        ("127.0.0.1".to_string(), alive_port),
    ]);
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();

    // 连续发几个包：如果 relay 因为 ICMP 端口不可达而退出，
    // 后面的包就不会被转发
    for i in 0..5u8 {
        sender
            .send_to(&make_packet(i), format!("127.0.0.1:{relay_port}"))
            .unwrap();
        thread::sleep(Duration::from_millis(30));
    }

    let got = recv_with_timeout(&alive, Duration::from_secs(3));
    relay.stop_and_join();

    assert!(
        got.is_some(),
        "有目标不可达时，relay 仍应继续向其他目标转发"
    );
}

#[test]
fn 短包也应被转发但会被统计() {
    // relay 的职责是透明转发，不做校验；短包照样转，
    // 由下游（解析器）决定怎么处理。但统计里要能看出来。
    let (sub, sub_port) = bind_ephemeral();

    // 这个用例需要读到 relay 的统计，所以不用 start_relay，改为手工驱动
    let stats_holder = Arc::new(std::sync::Mutex::new(Stats::default()));
    let stats2 = Arc::clone(&stats_holder);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);

    // 主线程绑定后交接给 relay 线程，避免端口竞态
    let (socket, port) = bind_relay_socket();
    let handle = thread::spawn(move || {
        let mut relay = Relay::from_socket(socket, &[("127.0.0.1".to_string(), sub_port)])
            .unwrap_or_else(|e| panic!("relay 接管套接字失败: {e}"));
        relay
            .set_read_timeout(Some(Duration::from_millis(20)))
            .expect("设置读取超时失败");
        loop {
            let _ = relay.pump_once();
            *stats2.lock().unwrap() = relay.stats();
            if stop2.load(Ordering::Relaxed) {
                return;
            }
        }
    });
    // relay 启动即已持有套接字，不需要等待"绑定成功"
    thread::sleep(Duration::from_millis(50));

    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let short = vec![0xAAu8; 10];
    sender.send_to(&short, format!("127.0.0.1:{port}")).unwrap();

    let got = recv_with_timeout(&sub, Duration::from_secs(2));
    // 给 relay 一点时间把统计写出来
    thread::sleep(Duration::from_millis(200));

    stop.store(true, Ordering::Relaxed);
    let _ = handle.join();

    assert_eq!(got.as_deref(), Some(short.as_slice()), "短包也应原样转发");
    let s = *stats_holder.lock().unwrap();
    assert!(s.packets >= 1, "应统计到包, 实际 {}", s.packets);
    assert!(
        s.short_packets >= 1,
        "应统计到短包, 实际 {}",
        s.short_packets
    );
    assert!(s.last_sender.is_some(), "应记录来源地址");
}

#[test]
fn 统计速率计算正确() {
    let stats = Stats {
        packets: 300,
        bytes: 300 * 324,
        short_packets: 0,
        last_sender: None,
    };
    let rate = Rate::from(&stats, Duration::from_secs(5));
    assert!((rate.packets_per_sec - 60.0).abs() < 1e-3, "应为 60 包/秒");
    // 324 字节 * 300 包 * 8 bit = 777600 bit，5 秒 → 155.52 kbps
    assert!((rate.kbps - 155.52).abs() < 0.01, "实际 {}", rate.kbps);

    // 零耗时不产生 NaN / inf
    let rate0 = Rate::from(&stats, Duration::ZERO);
    assert_eq!(rate0.packets_per_sec, 0.0);
    assert_eq!(rate0.kbps, 0.0);
}

#[test]
fn 端口解析的边界情况() {
    assert_eq!(
        parse_host_port("127.0.0.1:5300").unwrap(),
        ("127.0.0.1".to_string(), 5300)
    );
    assert_eq!(
        parse_host_port("localhost:5301").unwrap(),
        ("localhost".to_string(), 5301)
    );
    // IPv6 字面量
    assert_eq!(
        parse_host_port("[::1]:5301").unwrap(),
        ("::1".to_string(), 5301)
    );
    assert!(parse_host_port("no-port").is_err());
    assert!(parse_host_port("host:99999").is_err());
    assert!(parse_host_port(":5300").is_err());
}

#[test]
fn 绑定被占用的端口应明确失败而不是静默成功() {
    // 这条性质很重要：如果静默"绑定成功但收不到包"，
    // 用户会以为是游戏没发包，排查成本极高。
    let held = ReservedPort::new();
    let addr = format!("127.0.0.1:{}", held.port()).parse().unwrap();
    let r = Relay::bind(addr, &[("127.0.0.1".to_string(), 5302)]);
    assert!(r.is_err(), "端口已被占用时必须返回错误");
}
