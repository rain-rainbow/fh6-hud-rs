# FH6 遥测链路探针：被动监听 5301 / 5302，判断包走到哪一环就断了
#
# 设计要点：
#  * 全部以 SO_REUSEADDR 绑定，不独占端口、不转发、不干扰 relay 与 HUD；
#  * 在 Windows 上，多个 socket 同绑一个地址时只有第一个绑定者收到单播包，
#    因此本探针能否收到取决于绑定顺序 —— 这一点会在输出里明确提示，避免误判；
#  * 对照法：比较 5301 与 5302 的结果即可定位断点。
#
# 符号一律用 ASCII（[OK] / [X] / [!]），避免控制台代码页造成乱码。

try {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $OutputEncoding = [System.Text.Encoding]::UTF8
} catch { }

$ErrorActionPreference = 'Stop'

function New-ReuseSocket([int]$port) {
    $s = New-Object System.Net.Sockets.Socket(
        [System.Net.Sockets.AddressFamily]::InterNetwork,
        [System.Net.Sockets.SocketType]::Dgram,
        [System.Net.Sockets.ProtocolType]::Udp)
    # ReuseAddress 必须在 Bind 之前设置
    $s.SetSocketOption([System.Net.Sockets.SocketOptionLevel]::Socket,
                       [System.Net.Sockets.SocketOptionName]::ReuseAddress, $true)
    $s.Bind((New-Object System.Net.IPEndPoint([System.Net.IPAddress]::Loopback, $port)))
    return $s
}

Write-Output "=== FH6 telemetry link probe (passive listen, no forwarding) ==="
Write-Output ""

$ports = @(5301, 5302)
$socks = @{}
foreach ($p in $ports) {
    try {
        $socks[$p] = New-ReuseSocket $p
        Write-Output ("  bound 127.0.0.1:{0} (SO_REUSEADDR)" -f $p)
    } catch {
        Write-Output ("  [X] cannot bind 127.0.0.1:{0} : {1}" -f $p, $_.Exception.Message)
    }
}
if ($socks.Count -eq 0) {
    Write-Output ""
    Write-Output "No port could be bound. Exiting."
    exit 1
}

Write-Output ""
Write-Output "NOTE: on Windows a unicast packet is delivered only to the FIRST binder."
Write-Output "      If relay or fh6-hud started before this probe, this probe may see"
Write-Output "      nothing even though the link works. In that case read relay's own"
Write-Output "      statistics output instead of trusting this probe."
Write-Output ""
Write-Output "Listening for 15 seconds. Please ACTUALLY DRIVE in the game (not menus)."
Write-Output ""

foreach ($p in $socks.Keys) { $socks[$p].ReceiveTimeout = 500 }

$stats = @{}
foreach ($p in $socks.Keys) {
    $stats[$p] = @{ Count = 0; Bytes = 0; First = $null; Src = $null }
}

$deadline = (Get-Date).AddSeconds(15)
$buf = New-Object byte[] 2048
while ((Get-Date) -lt $deadline) {
    foreach ($p in @($socks.Keys)) {
        try {
            $ep = New-Object System.Net.IPEndPoint([System.Net.IPAddress]::Any, 0)
            $n = $socks[$p].ReceiveFrom($buf, [ref]$ep)
            $st = $stats[$p]
            $st.Count++
            $st.Bytes += $n
            if ($null -eq $st.First) { $st.First = $n; $st.Src = $ep.ToString() }
        } catch [System.Net.Sockets.SocketException] {
            # 超时属正常
        }
    }
}

Write-Output "=== Result (15 second window) ==="
foreach ($p in $ports) {
    if ($socks.ContainsKey($p)) {
        $st = $stats[$p]
        if ($st.Count -gt 0) {
            $pps = [math]::Round($st.Count / 15.0, 1)
            Write-Output ("  [OK] 127.0.0.1:{0} : {1} packets ({2}/s), first {3} bytes, from {4}" -f $p, $st.Count, $pps, $st.First, $st.Src)
        } else {
            Write-Output ("  [--] 127.0.0.1:{0} : 0 packets" -f $p)
        }
    }
}

Write-Output ""
Write-Output "=== Diagnosis ==="
$c1 = if ($socks.ContainsKey(5301)) { $stats[5301].Count } else { -1 }
$c2 = if ($socks.ContainsKey(5302)) { $stats[5302].Count } else { -1 }
if ($c1 -gt 0 -and $c2 -gt 0) {
    Write-Output "  [OK] Packets on both ports -> relay works and the link is up."
    Write-Output "       If fh6-hud still shows NO DATA, something else grabbed 5302 first."
} elseif ($c1 -gt 0 -and $c2 -eq 0) {
    Write-Output "  [!] Packets on 5301 but none on 5302 -> the relay is not forwarding."
    Write-Output "      Check that fh6-relay is running and its --to list includes 127.0.0.1:5302."
} elseif ($c1 -eq 0 -and $c2 -eq 0) {
    Write-Output "  [X] No packets on either port -> the game is not sending telemetry. Check:"
    Write-Output "      1) In game: Settings > HUD and Gameplay: Data Out = On"
    Write-Output "      2) Data Out IP Address = 127.0.0.1, Data Out IP Port = 5301"
    Write-Output "      3) Packets are sent ONLY while actively driving (not menus/pause/replay)"
} else {
    Write-Output "  Inconclusive (some ports failed to bind)."
}

foreach ($p in $socks.Keys) { $socks[$p].Dispose() }
