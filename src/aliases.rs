//! 队名别名表。
//!
//! 作用有两个：
//! 1. `canonical_name("天禄") -> "TYLOO"`：把中文/俗称/缩写解析成 PandaScore 认得的英文规范名；
//! 2. `zh_name("Team Vitality") -> Some("小蜜蜂")`：给统一契约里的 `name_zh` 填中文称谓。
//!
//! 原则：**只收录稳定、社区广泛使用的称谓**，查不到一律返回 `None`
//! （`name_zh` 输出 `null`），绝不机器翻译或猜测。

/// (别名/中文/缩写, 英文规范名, 中文展示名)
///
/// 第一列一律小写；查表时输入会先 `trim + lowercase`。
/// 第三列为 `None` 表示该队没有广泛通用的中文称谓，`name_zh` 输出 `null`。
const TABLE: &[(&str, &str, Option<&str>)] = &[
    // ---- 顶级战队 ----
    ("navi", "Natus Vincere", Some("NAVI")),
    ("natus vincere", "Natus Vincere", Some("NAVI")),
    ("g2", "G2 Esports", Some("G2")),
    ("g2 esports", "G2 Esports", Some("G2")),
    ("faze", "FaZe Clan", Some("FaZe")),
    ("faze clan", "FaZe Clan", Some("FaZe")),
    ("小蜜蜂", "Team Vitality", Some("小蜜蜂")),
    ("vitality", "Team Vitality", Some("小蜜蜂")),
    ("team vitality", "Team Vitality", Some("小蜜蜂")),
    ("绿龙", "Team Spirit", Some("绿龙")),
    ("spirit", "Team Spirit", Some("绿龙")),
    ("team spirit", "Team Spirit", Some("绿龙")),
    ("老鼠", "MOUZ", Some("老鼠")),
    ("mouz", "MOUZ", Some("老鼠")),
    ("红星", "Astralis", Some("红星")),
    ("astralis", "Astralis", Some("红星")),
    ("李逵", "Team Liquid", Some("李逵")),
    ("liquid", "Team Liquid", Some("李逵")),
    ("team liquid", "Team Liquid", Some("李逵")),
    ("天禄", "TYLOO", Some("天禄")),
    ("tyloo", "TYLOO", Some("天禄")),
    ("ra", "Rare Atom", Some("RA")),
    ("rare atom", "Rare Atom", Some("RA")),
    ("蒙古", "The MongolZ", Some("蒙古队")),
    ("mongolz", "The MongolZ", Some("蒙古队")),
    ("the mongolz", "The MongolZ", Some("蒙古队")),
    // ---- 常见一线队（无通用中文称谓者，name_zh 为 null）----
    ("heroic", "Heroic", None),
    ("c9", "Cloud9", Some("C9")),
    ("cloud9", "Cloud9", Some("C9")),
    ("nip", "Ninjas in Pyjamas", Some("NIP")),
    ("ninjas in pyjamas", "Ninjas in Pyjamas", Some("NIP")),
    ("vp", "Virtus.pro", Some("VP")),
    ("virtus.pro", "Virtus.pro", Some("VP")),
    ("fnatic", "fnatic", None),
    ("furia", "FURIA Esports", Some("黑豹")),
    ("黑豹", "FURIA Esports", Some("黑豹")),
    ("mibr", "MIBR", None),
    ("col", "Complexity", Some("COL")),
    ("complexity", "Complexity", Some("COL")),
    ("3dmax", "3DMAX", None),
    ("big", "BIG", None),
    ("gl", "GamerLegion", Some("GL")),
    ("gamerlegion", "GamerLegion", Some("GL")),
    ("ef", "Eternal Fire", Some("EF")),
    ("eternal fire", "Eternal Fire", Some("EF")),
    ("猎鹰", "Falcons", Some("猎鹰")),
    ("falcons", "Falcons", Some("猎鹰")),
    ("极光", "Aurora", Some("极光")),
    ("aurora", "Aurora", Some("极光")),
    ("pain", "paiN Gaming", Some("paiN")),
    ("pain gaming", "paiN Gaming", Some("paiN")),
    ("imperial", "Imperial", None),
    ("lvg", "Lynn Vision", Some("LVG")),
    ("lynn vision", "Lynn Vision", Some("LVG")),
];

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// 把中文/俗称/缩写解析成英文规范名。查不到返回 `None`。
pub fn canonical_name(input: &str) -> Option<&'static str> {
    let key = norm(input);
    if key.is_empty() {
        return None;
    }
    TABLE
        .iter()
        .find(|(alias, canon, _)| *alias == key || canon.to_lowercase() == key)
        .map(|(_, canon, _)| *canon)
}

/// 查英文规范名对应的中文称谓。没有通用称谓时返回 `None`（对外输出 `null`）。
pub fn zh_name(name: &str) -> Option<&'static str> {
    let key = norm(name);
    if key.is_empty() {
        return None;
    }
    TABLE
        .iter()
        .find(|(_, canon, _)| canon.to_lowercase() == key)
        .and_then(|(_, _, zh)| *zh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_nickname_resolves_to_canonical() {
        assert_eq!(canonical_name("天禄"), Some("TYLOO"));
        assert_eq!(canonical_name("小蜜蜂"), Some("Team Vitality"));
        assert_eq!(canonical_name("绿龙"), Some("Team Spirit"));
        assert_eq!(canonical_name("李逵"), Some("Team Liquid"));
        assert_eq!(canonical_name("老鼠"), Some("MOUZ"));
        assert_eq!(canonical_name("蒙古"), Some("The MongolZ"));
    }

    #[test]
    fn english_and_acronyms_resolve() {
        assert_eq!(canonical_name("NAVI"), Some("Natus Vincere"));
        assert_eq!(canonical_name("  g2 "), Some("G2 Esports"));
        assert_eq!(canonical_name("Team Vitality"), Some("Team Vitality"));
        assert_eq!(canonical_name("vp"), Some("Virtus.pro"));
    }

    #[test]
    fn unknown_team_returns_none() {
        assert_eq!(canonical_name("Some Random Team"), None);
        assert_eq!(canonical_name(""), None);
        assert_eq!(canonical_name("   "), None);
    }

    #[test]
    fn zh_name_lookup() {
        assert_eq!(zh_name("Team Vitality"), Some("小蜜蜂"));
        assert_eq!(zh_name("TYLOO"), Some("天禄"));
        assert_eq!(zh_name("Natus Vincere"), Some("NAVI"));
        // 没有通用中文称谓的队必须返回 None，而不是硬翻
        assert_eq!(zh_name("Heroic"), None);
        assert_eq!(zh_name("fnatic"), None);
        assert_eq!(zh_name("Unknown Team"), None);
    }
}
