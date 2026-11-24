//! Repérage des données sensibles dans une ligne de texte : courriels, adresses IP,
//! cartes bancaires (contrôle de Luhn), IBAN (contrôle modulo 97), clés d'API,
//! numéros de téléphone français. Pur, sans OCR : testé ici.

use std::ops::Range;
use std::sync::LazyLock;

use regex_lite::Regex;
use serde::{Deserialize, Serialize};

/// Nature d'une donnée sensible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SensitiveKind {
    Email,
    Ipv4,
    Ipv6,
    CreditCard,
    Iban,
    ApiKey,
    Phone,
}

impl SensitiveKind {
    /// Nom affiché.
    pub fn label(self) -> &'static str {
        match self {
            Self::Email => "Courriel",
            Self::Ipv4 => "Adresse IPv4",
            Self::Ipv6 => "Adresse IPv6",
            Self::CreditCard => "Carte bancaire",
            Self::Iban => "IBAN",
            Self::ApiKey => "Clé d'API",
            Self::Phone => "Téléphone",
        }
    }
}

/// Motif à chercher et contrôle supplémentaire du texte trouvé.
struct Rule {
    kind: SensitiveKind,
    regex: Regex,
    check: fn(&str) -> bool,
}

fn always(_: &str) -> bool {
    true
}

/// Règles par ordre de priorité : quand deux passages se chevauchent, le premier
/// trouvé par une règle plus haute l'emporte (un IBAN contient des chiffres qui
/// ressemblent à une carte, une clé peut contenir un « @ »…).
static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let rule = |kind, pattern: &str, check: fn(&str) -> bool| Rule {
        kind,
        regex: Regex::new(pattern).expect("motif valide"),
        check,
    };
    vec![
        // Préfixes connus : OpenAI / Anthropic (sk-…), AWS (AKIA, ASIA), GitHub
        // (ghp_, gho_, ghu_, ghs_, ghr_, github_pat_), Slack (xoxb-…), Stripe
        // (sk_live_…), Google (AIza…), jetons JWT.
        rule(
            SensitiveKind::ApiKey,
            r"\b(?:sk-[A-Za-z0-9_-]{16,}|(?:AKIA|ASIA)[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{20,}|xox[abposr]-[A-Za-z0-9-]{10,}|[rsp]k_(?:live|test)_[A-Za-z0-9]{16,}|AIza[0-9A-Za-z_-]{35}|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})",
            always,
        ),
        rule(
            SensitiveKind::Iban,
            r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]){11,30}\b",
            iban_ok,
        ),
        rule(
            SensitiveKind::CreditCard,
            r"\b\d(?:[ -]?\d){12,18}\b",
            card_ok,
        ),
        rule(
            SensitiveKind::Email,
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}\b",
            always,
        ),
        rule(
            SensitiveKind::Phone,
            r"(?:(?:\+|\b00)33[ .-]?(?:\(0\)[ .-]?)?|\b0)[1-9](?:[ .-]?\d{2}){4}\b",
            always,
        ),
        rule(
            SensitiveKind::Ipv6,
            r"(?i)(?:[0-9a-f]{0,4}:){2,7}[0-9a-f]{0,4}(?:/\d{1,3})?",
            ipv6_ok,
        ),
        rule(
            SensitiveKind::Ipv4,
            r"\b(?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\b(?:/\d{1,2}\b)?",
            always,
        ),
        // Jeton quelconque à forte entropie (clés sans préfixe connu).
        rule(
            SensitiveKind::ApiKey,
            r"[A-Za-z0-9+/_=-]{20,}",
            high_entropy_token,
        ),
    ]
});

/// Passages sensibles de `text` (indices d'octets), triés par position, sans
/// chevauchement.
pub fn find(text: &str) -> Vec<(SensitiveKind, Range<usize>)> {
    let mut found: Vec<(SensitiveKind, Range<usize>)> = Vec::new();
    for rule in RULES.iter() {
        for m in rule.regex.find_iter(text) {
            let range = m.range();
            let overlaps = found
                .iter()
                .any(|(_, r)| r.start < range.end && range.start < r.end);
            if !overlaps && (rule.check)(m.as_str()) {
                found.push((rule.kind, range));
            }
        }
    }
    found.sort_by_key(|(_, r)| r.start);
    found
}

/// Contrôle de Luhn sur les chiffres de `s` (séparateurs ignorés).
pub fn luhn(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() < 2 {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                let d = d * 2;
                if d > 9 { d - 9 } else { d }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Numéro de carte : 13 à 19 chiffres, séparateurs réguliers, Luhn valide, et pas
/// une suite d'un seul chiffre.
fn card_ok(s: &str) -> bool {
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    let first = digits.chars().next();
    (13..=19).contains(&digits.len()) && !digits.chars().all(|c| Some(c) == first) && luhn(&digits)
}

/// IBAN : 15 à 34 caractères, clé modulo 97 (ISO 13616).
pub fn iban_ok(s: &str) -> bool {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !(15..=34).contains(&compact.len()) || !compact.is_ascii() {
        return false;
    }
    let (head, tail) = compact.split_at(4);
    let mut rem: u32 = 0;
    for c in tail.chars().chain(head.chars()) {
        let value = match c {
            '0'..='9' => c as u32 - '0' as u32,
            'A'..='Z' => c as u32 - 'A' as u32 + 10,
            _ => return false,
        };
        rem = if value >= 10 {
            (rem * 100 + value) % 97
        } else {
            (rem * 10 + value) % 97
        };
    }
    rem == 1
}

/// Adresse IPv6 valide, avec au moins trois groupes non vides (évite « 10:30 »,
/// « a::b » dans du code…).
fn ipv6_ok(s: &str) -> bool {
    let addr = s.split('/').next().unwrap_or(s);
    let groups = addr.split(':').filter(|g| !g.is_empty()).count();
    groups >= 3 && addr.parse::<std::net::Ipv6Addr>().is_ok()
}

/// Entropie de Shannon, en bits par caractère.
pub fn entropy(s: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0u32) += 1;
    }
    let n = s.chars().count() as f64;
    counts
        .values()
        .map(|&k| {
            let p = f64::from(k) / n;
            -p * p.log2()
        })
        .sum()
}

/// Jeton aléatoire : majuscules, minuscules et chiffres mélangés, entropie élevée.
/// Écarte les mots longs, les chemins et les empreintes hexadécimales (commits).
fn high_entropy_token(s: &str) -> bool {
    let upper = s.chars().any(|c| c.is_ascii_uppercase());
    let lower = s.chars().any(|c| c.is_ascii_lowercase());
    let digit = s.chars().filter(char::is_ascii_digit).count();
    upper && lower && digit >= 2 && entropy(s) >= 3.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(SensitiveKind, &str)> {
        find(text).into_iter().map(|(k, r)| (k, &text[r])).collect()
    }

    #[test]
    fn luhn_accepts_test_cards_only() {
        assert!(luhn("4111 1111 1111 1111"));
        assert!(luhn("5555555555554444"));
        assert!(luhn("378282246310005"));
        assert!(!luhn("4111 1111 1111 1112"));
        assert!(!luhn("1"));
    }

    #[test]
    fn iban_checksum() {
        assert!(iban_ok("FR76 3000 6000 0112 3456 7890 189"));
        assert!(iban_ok("DE89370400440532013000"));
        assert!(!iban_ok("FR76 3000 6000 0112 3456 7890 188"));
        assert!(!iban_ok("FR76"));
    }

    #[test]
    fn emails() {
        assert_eq!(
            kinds("Écrire à jean.dupont+test@exemple.fr demain"),
            vec![(SensitiveKind::Email, "jean.dupont+test@exemple.fr")]
        );
        assert!(kinds("pas un courriel : a@b").is_empty());
    }

    #[test]
    fn ip_addresses() {
        assert_eq!(
            kinds("serveur 192.168.1.20 et 10.0.0.0/8"),
            vec![
                (SensitiveKind::Ipv4, "192.168.1.20"),
                (SensitiveKind::Ipv4, "10.0.0.0/8")
            ]
        );
        assert!(kinds("version 1.2.300.4").is_empty());
        assert_eq!(
            kinds("hôte 2001:db8::8a2e:370:7334 ok"),
            vec![(SensitiveKind::Ipv6, "2001:db8::8a2e:370:7334")]
        );
        assert_eq!(
            kinds("lien fe80::1ff:fe23:4567:890a%en0"),
            vec![(SensitiveKind::Ipv6, "fe80::1ff:fe23:4567:890a")]
        );
        assert!(kinds("rendez-vous à 10:30:45").is_empty());
    }

    #[test]
    fn credit_cards_need_luhn() {
        assert_eq!(
            kinds("Carte 4111 1111 1111 1111 exp 12/29"),
            vec![(SensitiveKind::CreditCard, "4111 1111 1111 1111")]
        );
        assert_eq!(
            kinds("n° 4111-1111-1111-1111"),
            vec![(SensitiveKind::CreditCard, "4111-1111-1111-1111")]
        );
        assert!(kinds("commande 4111 1111 1111 1112").is_empty());
        assert!(kinds("0000000000000000").is_empty());
    }

    #[test]
    fn iban_wins_over_card_digits() {
        assert_eq!(
            kinds("IBAN : FR76 3000 6000 0112 3456 7890 189"),
            vec![(SensitiveKind::Iban, "FR76 3000 6000 0112 3456 7890 189")]
        );
    }

    #[test]
    fn api_keys() {
        // Clés d'exemple coupées en deux : une clé entière dans le source ferait
        // bloquer le dépôt par les détecteurs de secrets (GitHub).
        let keys = [
            ["sk-proj-", "AbCdEf0123456789xyzXYZ"].concat(),
            ["AKIA", "IOSFODNN7EXAMPLE"].concat(),
            ["ghp_", "abcdefghijklmnopqrstuvwxyz0123456789"].concat(),
            ["xoxb-", "123456789012-abcdefABCDEF"].concat(),
            ["sk_live_", "4eC39HqLyjWDarjtT1zdp7dc"].concat(),
            ["AIza", "SyA1234567890abcdefghijklmnopqrstuv"].concat(),
        ];
        for key in &keys {
            let key = key.as_str();
            let text = format!("clé = {key};");
            assert_eq!(kinds(&text), vec![(SensitiveKind::ApiKey, key)], "{key}");
        }
    }

    #[test]
    fn generic_tokens_need_entropy() {
        assert_eq!(
            kinds("token: Zx9Qp2Lm7Rt4Vb8Nc1Kd5Wf3"),
            vec![(SensitiveKind::ApiKey, "Zx9Qp2Lm7Rt4Vb8Nc1Kd5Wf3")]
        );
        // Mot long, chemin, empreinte de commit : rien.
        assert!(kinds("anticonstitutionnellement").is_empty());
        assert!(kinds("src/vision/sensitive_regions_test").is_empty());
        assert!(kinds("commit 3f786850e387550fdab836ed7e6dc881de23001b").is_empty());
    }

    #[test]
    fn french_phone_numbers() {
        for phone in [
            "06 12 34 56 78",
            "01.23.45.67.89",
            "0612345678",
            "+33 6 12 34 56 78",
            "+33 (0)1 23 45 67 89",
            "0033 6 12 34 56 78",
        ] {
            let text = format!("Tél. {phone} !");
            assert_eq!(kinds(&text), vec![(SensitiveKind::Phone, phone)], "{phone}");
        }
        assert!(kinds("réf. 00 12 34 56 78").is_empty());
    }

    #[test]
    fn several_hits_are_sorted_without_overlap() {
        let text = "a@b.io 8.8.8.8 06 12 34 56 78";
        let found = find(text);
        assert_eq!(found.len(), 3);
        assert!(found.windows(2).all(|w| w[0].1.end <= w[1].1.start));
    }
}
