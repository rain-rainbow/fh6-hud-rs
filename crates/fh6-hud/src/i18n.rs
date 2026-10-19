//! 界面文本本地化。
//!
//! # 为什么单独成模块
//!
//! 之前所有标签都是硬编码英文字面量散落在 [`crate::hud`] 的各绘制方法里
//! （`"THROTTLE"`、`"FLOW"`、`"SPIN SAVE!"` …）。要支持中英切换，有两种做法：
//!
//! 1. 在每个 `self.text("THROTTLE", …)` 处写 `if zh { "油门" } else { "THROTTLE" }`；
//! 2. 把「枚举 → 显示文本」的映射集中到一处。
//!
//! 采用第 2 种。好处是：翻译表可以被**穷尽性检查**（`match` 覆盖所有变体，
//! 将来给 [`DriftState`](fh6_core::drift::DriftState) 加变体时编译器会报错），
//! 而且能直接写单元测试验证「中文模式下确实没有残留英文名词」。
//!
//! # 术语取舍
//!
//! * 保留不译：`km/h`、`G`、`RPM` 的「转速」（RPM 数值本身保留）、挡位数字；
//! * `FLOW` 在中文里没有单字对应，译作「流畅度」，其质量等级「流畅」；
//! * 漂移风格（Style）是玩家社区用语，直译会失去辨识度，采用「深线」
//!   「踢尾」这类中文漂移圈常用说法。

use fh6_core::drift::{DriftState, FlowQuality, OperationPopup, Popup, RiskLabel, Style};

use crate::shared::LinkState;

use fh6_config::settings::Language;

/// HUD 上的固定文案键。
///
/// 用枚举而不是直接传字符串，是为了让「漏翻一处」在编译期或测试期暴露。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiText {
    /// 输入面板标题。
    InputPanel,
    /// 油门。
    Throttle,
    /// 刹车。
    Brake,
    /// 离合。
    Clutch,
    /// 手刹。
    Handbrake,
    /// 方向盘（后接方向）。
    Steer,
    /// 方向：左。
    SteerLeft,
    /// 方向：右。
    SteerRight,
    /// 漂移面板标题。
    DriftPanel,
    /// 后轮左。
    RearLeft,
    /// 后轮右。
    RearRight,
    /// 后轴平均。
    RearAvg,
    /// 转速。
    Rpm,
    /// 反打。
    Counter,
    /// 流畅度（面板行标签）。
    FlowRow,
    /// 质量。
    Quality,
    /// 风险。
    Risk,
    /// 等级。
    Level,
    /// 风格。
    Style,
    /// 状态。
    State,
    /// G 表标题。
    Gmeter,
    /// 速度单位。
    SpeedUnit,
}

/// 本地化文本提供者。
#[derive(Debug, Clone, Copy)]
pub struct Labels {
    lang: Language,
}

impl Labels {
    /// 按语言构造。
    pub fn new(lang: Language) -> Self {
        Self { lang }
    }

    /// 当前语言。
    pub fn lang(&self) -> Language {
        self.lang
    }

    /// 是否中文。
    pub fn is_zh(&self) -> bool {
        matches!(self.lang, Language::Zh)
    }

    /// 字形高度相对拉丁字母的倍率。
    ///
    /// 拉丁大写字母只占字号的约 0.72（cap height），而汉字**占满整个字身框**
    /// （约 1.0 个字号），且上下留白更少。用同一个行距排版时中文会多占约
    /// 25%~30% 的高度，直接套用英文的行距就会让标签压到下方的条形上。
    ///
    /// 这个系数用于计算「标签实际占多高」（[`crate::hud`] 的 `glyph_h`）。
    pub fn glyph_factor(&self) -> f32 {
        match self.lang {
            Language::Zh => 1.28,
            Language::En => 1.0,
        }
    }

    /// 行距相对拉丁字母的倍率。
    ///
    /// 比 [`Labels::glyph_factor`] 更大：除了字更高，中文还需要额外的行间隙
    /// 才不显拥挤。用于状态栏这类「一行行排列」的布局。
    pub fn line_factor(&self) -> f32 {
        match self.lang {
            Language::Zh => 1.50,
            Language::En => 1.0,
        }
    }

    /// 固定文案。
    pub fn ui(&self, key: UiText) -> &'static str {
        match self.lang {
            Language::Zh => match key {
                UiText::InputPanel => "输入",
                UiText::Throttle => "油门",
                UiText::Brake => "刹车",
                UiText::Clutch => "离合",
                UiText::Handbrake => "手刹",
                UiText::Steer => "方向",
                UiText::SteerLeft => "左",
                UiText::SteerRight => "右",
                UiText::DriftPanel => "漂移",
                UiText::RearLeft => "后左",
                UiText::RearRight => "后右",
                UiText::RearAvg => "后轴",
                UiText::Rpm => "转速",
                UiText::Counter => "反打",
                UiText::FlowRow => "流畅",
                UiText::Quality => "质量",
                UiText::Risk => "风险",
                UiText::Level => "等级",
                UiText::Style => "风格",
                UiText::State => "状态",
                UiText::Gmeter => "G",
                // 单位按国际惯例保留
                UiText::SpeedUnit => "km/h",
            },
            Language::En => match key {
                UiText::InputPanel => "INPUT",
                UiText::Throttle => "THROTTLE",
                UiText::Brake => "BRAKE",
                UiText::Clutch => "CLUTCH",
                UiText::Handbrake => "HANDBRAKE",
                UiText::Steer => "STEER",
                UiText::SteerLeft => "L",
                UiText::SteerRight => "R",
                UiText::DriftPanel => "DRIFT",
                UiText::RearLeft => "REAR RL",
                UiText::RearRight => "REAR RR",
                UiText::RearAvg => "REAR AVG",
                UiText::Rpm => "RPM",
                UiText::Counter => "COUNTER",
                UiText::FlowRow => "FLOW",
                UiText::Quality => "QUALITY",
                UiText::Risk => "RISK",
                UiText::Level => "LEVEL",
                UiText::Style => "STYLE",
                UiText::State => "STATE",
                UiText::Gmeter => "G",
                UiText::SpeedUnit => "km/h",
            },
        }
    }

    /// 漂移状态。
    pub fn drift_state(&self, s: DriftState) -> &'static str {
        match self.lang {
            Language::Zh => match s {
                DriftState::Grip => "抓地",
                DriftState::Entry => "入弯",
                DriftState::Hold => "保持",
                DriftState::Angle => "角度",
                DriftState::Smoke => "冒烟",
                DriftState::Spin => "打转",
            },
            Language::En => s.label(),
        }
    }

    /// 流畅质量等级。
    pub fn flow_quality(&self, q: FlowQuality) -> &'static str {
        match self.lang {
            Language::Zh => match q {
                FlowQuality::Build => "蓄力",
                FlowQuality::Flow => "流畅",
                FlowQuality::Chase => "追赶",
                FlowQuality::Smooth => "顺滑",
                FlowQuality::Locked => "锁定",
            },
            Language::En => q.label(),
        }
    }

    /// 风险等级。
    pub fn risk(&self, r: RiskLabel) -> &'static str {
        match self.lang {
            Language::Zh => match r {
                RiskLabel::Safe => "安全",
                RiskLabel::Edge => "临界",
                RiskLabel::Risk => "风险",
                RiskLabel::Max => "极限",
            },
            Language::En => r.label(),
        }
    }

    /// 漂移风格。
    pub fn style(&self, s: Style) -> &'static str {
        match self.lang {
            Language::Zh => match s {
                Style::Grip => "抓地",
                Style::Entry => "入弯",
                Style::Clean => "干净",
                Style::Smooth => "顺滑",
                Style::Deep => "深线",
                Style::Locked => "锁定",
                Style::Wild => "狂野",
                Style::Edge => "临界",
                Style::Danger => "危险",
                Style::Save => "救车",
                Style::Kick => "踢尾",
                Style::Brake => "刹车",
            },
            Language::En => s.label(),
        }
    }

    /// 事件弹窗标题。
    pub fn popup(&self, p: Popup) -> &'static str {
        match self.lang {
            Language::Zh => match p {
                Popup::SpinSave => "救车成功！",
                Popup::BigAngle => "大角度！",
                Popup::LongHold => "长保持！",
                Popup::FlowLock => "流畅锁定！",
                Popup::DeepLine => "深线！",
                Popup::LimitEdge => "临界！",
                Popup::SmokeRun => "冒烟跑！",
                Popup::Entry => "入弯！",
            },
            Language::En => p.label(),
        }
    }

    /// 操作提示弹窗。
    pub fn operation(&self, o: OperationPopup) -> &'static str {
        match self.lang {
            Language::Zh => match o {
                OperationPopup::Handbrake => "手刹",
                OperationPopup::Clutch => "离合",
                OperationPopup::Brake => "刹车",
            },
            Language::En => o.label(),
        }
    }

    /// 连接状态。
    pub fn link(&self, l: LinkState) -> &'static str {
        match self.lang {
            Language::Zh => match l {
                LinkState::NoData => "无数据",
                LinkState::Live => "在线",
                LinkState::Stale => "延迟",
            },
            Language::En => l.label(),
        }
    }
}

impl Default for Labels {
    fn default() -> Self {
        Self::new(Language::default())
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    fn zh() -> Labels {
        Labels::new(Language::Zh)
    }
    fn en() -> Labels {
        Labels::new(Language::En)
    }

    /// 中文模式下，所有文案必须含中文或为公认保留单位。
    ///
    /// 这条测试就是「对中文用户友好」这个需求的验收标准：将来新增条目若
    /// 忘了翻译（直接返回英文），这里会立刻失败。
    #[test]
    fn 中文模式下不得残留英文名词() {
        let l = zh();
        // 允许保留的：单位与品牌性字符
        let allowed = ["km/h", "G"];
        let mut samples: Vec<(&str, &'static str)> = Vec::new();
        for key in [
            UiText::InputPanel,
            UiText::Throttle,
            UiText::Brake,
            UiText::Clutch,
            UiText::Handbrake,
            UiText::Steer,
            UiText::DriftPanel,
            UiText::RearLeft,
            UiText::RearRight,
            UiText::RearAvg,
            UiText::Rpm,
            UiText::Counter,
            UiText::FlowRow,
            UiText::Quality,
            UiText::Risk,
            UiText::Level,
            UiText::Style,
            UiText::State,
            UiText::SpeedUnit,
        ] {
            samples.push(("ui", l.ui(key)));
        }
        for s in [
            DriftState::Grip,
            DriftState::Entry,
            DriftState::Hold,
            DriftState::Angle,
            DriftState::Smoke,
            DriftState::Spin,
        ] {
            samples.push(("state", l.drift_state(s)));
        }
        for q in [
            FlowQuality::Build,
            FlowQuality::Flow,
            FlowQuality::Chase,
            FlowQuality::Smooth,
            FlowQuality::Locked,
        ] {
            samples.push(("flow", l.flow_quality(q)));
        }
        for r in [
            RiskLabel::Safe,
            RiskLabel::Edge,
            RiskLabel::Risk,
            RiskLabel::Max,
        ] {
            samples.push(("risk", l.risk(r)));
        }
        for s in [
            Style::Grip,
            Style::Entry,
            Style::Clean,
            Style::Smooth,
            Style::Deep,
            Style::Locked,
            Style::Wild,
            Style::Edge,
            Style::Danger,
            Style::Save,
            Style::Kick,
            Style::Brake,
        ] {
            samples.push(("style", l.style(s)));
        }
        for p in [
            Popup::SpinSave,
            Popup::BigAngle,
            Popup::LongHold,
            Popup::FlowLock,
            Popup::DeepLine,
            Popup::LimitEdge,
            Popup::SmokeRun,
            Popup::Entry,
        ] {
            samples.push(("popup", l.popup(p)));
        }
        for o in [
            OperationPopup::Handbrake,
            OperationPopup::Clutch,
            OperationPopup::Brake,
        ] {
            samples.push(("op", l.operation(o)));
        }
        for s in [LinkState::NoData, LinkState::Live, LinkState::Stale] {
            samples.push(("link", l.link(s)));
        }

        for (kind, text) in samples {
            let has_cjk = text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
            let is_allowed = allowed.contains(&text);
            assert!(
                has_cjk || is_allowed,
                "中文模式下 {kind} 的文案 `{text}` 既不含中文也不是允许保留的单位"
            );
        }
    }

    /// 反过来：英文模式下不应出现中文。
    #[test]
    fn 英文模式下不得出现中文() {
        let l = en();
        let mut samples: Vec<&'static str> = Vec::new();
        for key in [UiText::InputPanel, UiText::Throttle, UiText::RearLeft] {
            samples.push(l.ui(key));
        }
        for s in [
            DriftState::Grip,
            DriftState::Entry,
            DriftState::Hold,
            DriftState::Angle,
            DriftState::Smoke,
            DriftState::Spin,
        ] {
            samples.push(l.drift_state(s));
        }
        for p in [Popup::SpinSave, Popup::BigAngle, Popup::Entry] {
            samples.push(l.popup(p));
        }
        for s in [LinkState::NoData, LinkState::Live, LinkState::Stale] {
            samples.push(l.link(s));
        }
        for text in samples {
            assert!(
                !text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "英文模式下出现中文: `{text}`"
            );
        }
    }

    #[test]
    fn 英文模式与core原有标签一致() {
        let l = en();
        // 英文模式下应完全复用 fh6-core 的既有标签，保证行为不变
        assert_eq!(l.drift_state(DriftState::Grip), DriftState::Grip.label());
        assert_eq!(
            l.flow_quality(FlowQuality::Locked),
            FlowQuality::Locked.label()
        );
        assert_eq!(l.risk(RiskLabel::Max), RiskLabel::Max.label());
        assert_eq!(l.style(Style::Wild), Style::Wild.label());
        assert_eq!(l.popup(Popup::DeepLine), Popup::DeepLine.label());
        assert_eq!(
            l.operation(OperationPopup::Clutch),
            OperationPopup::Clutch.label()
        );
        assert_eq!(l.link(LinkState::NoData), LinkState::NoData.label());
    }

    #[test]
    fn 中文关键翻译正确() {
        let l = zh();
        assert_eq!(l.ui(UiText::Throttle), "油门");
        assert_eq!(l.ui(UiText::Brake), "刹车");
        assert_eq!(l.ui(UiText::Handbrake), "手刹");
        assert_eq!(l.ui(UiText::RearLeft), "后左");
        assert_eq!(l.drift_state(DriftState::Grip), "抓地");
        assert_eq!(l.popup(Popup::SpinSave), "救车成功！");
        assert_eq!(l.link(LinkState::NoData), "无数据");
        assert_eq!(l.risk(RiskLabel::Safe), "安全");
    }

    #[test]
    fn 方向标识按语言切换() {
        assert_eq!(zh().ui(UiText::SteerLeft), "左");
        assert_eq!(zh().ui(UiText::SteerRight), "右");
        assert_eq!(en().ui(UiText::SteerLeft), "L");
        assert_eq!(en().ui(UiText::SteerRight), "R");
    }

    #[test]
    fn 默认语言为中文() {
        assert!(zh().is_zh());
        assert!(!en().is_zh());
        assert_eq!(Labels::default().lang(), Language::Zh);
    }
}
