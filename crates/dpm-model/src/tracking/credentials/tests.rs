use super::{check_text, check_url};

fn github() -> crate::ExternalIdentity {
    crate::ExternalIdentity {
        provider: crate::ExternalProvider::GitHub,
        instance: "github.com".into(),
        namespace: Some("o/r".into()),
        kind: crate::ExternalObjectKind::Issue,
        external_id: "6".into(),
    }
}

/// Both decisions must reject: the URL field may be stricter than a label, never looser.
fn rejected_by_both(text: &str) {
    assert!(check_text(text).is_err(), "label accepted {text}");
    assert!(check_url(text, &github()).is_err(), "url accepted {text}");
}

#[test]
fn userinfo_in_every_authority_spelling_is_rejected_by_label_and_url() {
    for text in [
        "//ghp_abc@github.com/o/r",
        "https:/tok@github.com/o/r",
        "https:\\\\tok@github.com\\o\\r",
        "user:ghp_secret@github.com",
        "ghp_abc@[2001:db8::1]/o/r",
        "https://tok@github.com",
        "https://git:pw@github.com/o/r",
        "tok@localhost:3000/x",
    ] {
        rejected_by_both(text);
    }
}

#[test]
fn secret_parameters_behind_any_separator_are_rejected_by_label_and_url() {
    for text in [
        "https://github.com/o/r/issues/6;token=abc",
        "https://github.com/o/r/issues/6/token=abc",
        "https://github.com/o/r/issues/6#access_token=abc",
        "https://github.com/o/r/issues/6?accessToken=abc",
        "https://github.com/o/r/issues/6?client_secret=abc",
    ] {
        rejected_by_both(text);
    }
}

#[test]
fn ordinary_names_anchors_and_ssh_remotes_are_not_credentials() {
    for label in [
        "max_tokens=4096",
        "secret_santa=on",
        "see #token-refresh and #api-key-setup",
        "clone git@github.com:o/r.git",
        "Contact user@example.com about #auth",
        "Ping @octocat on ops/dpm",
        "Why? Because x=1 and token= parsing",
        "Refactor auth/code key handling",
        "Review 1.5/2 of the plan (codeberg.org/ops/dpm)",
    ] {
        assert_eq!(check_text(label), Ok(()), "{label}");
    }
    for url in [
        "https://github.com/o/r/issues/6#token-refresh",
        "https://github.com/o/r/issues/6#api-key-setup",
        "https://github.com/ops/dpm/pull/7?tab=files",
        "https://github.com/ops/dpm/issues/42?tab=activity#top",
        "https://github.com:0443/ops/dpm/issues/42",
    ] {
        assert_eq!(check_text(url), Ok(()), "label {url}");
        assert_eq!(check_url(url, &github()), Ok(()), "url {url}");
    }
}

#[test]
fn urls_follow_the_structural_rules() {
    for url in [
        "https://github.com/x?%74oken=x",
        "https://github.com/o/r;v=1",
        "https://github.com/o/r#a=b",
        "https:///github.com/o/r",
        "ftp://github.com/o/r",
        "https://github.com.evil.example/o/r",
        "https://github.com/o/r/%40x",
    ] {
        assert!(check_url(url, &github()).is_err(), "accepted {url}");
    }
}

const SCHEMES: &[&str] = &[
    "https://",
    "http://",
    "HTTPS://",
    "https:/",
    "https:",
    "https:\\\\",
    "https:\\",
    "//",
    "\\\\",
    "ssh://",
];
const USERINFO: &[&str] = &[
    "ghp_abc",
    "user:ghp_secret",
    "user:",
    "tok%40x",
    "%74ok",
    "user%3Apw",
    "git:pw",
    "GHP_ABC",
];
const HOSTS: &[&str] = &[
    "github.com",
    "GitHub.com",
    "github.com:443",
    "[2001:db8::1]",
    "[::1]:8080",
    "127.0.0.1",
    "localhost:3000",
    "localhost",
    "intranet",
];
const TAILS: &[&str] = &["", "/o/r", "\\o\\r", "/o/r/issues/6", "?x=1", "#top"];
const AT_SIGNS: &[&str] = &["@", "%40", "%2540"];

#[test]
fn generated_userinfo_forms_are_rejected_by_label_and_url() {
    let mut count = 0;
    for scheme in SCHEMES {
        for userinfo in USERINFO {
            for at in AT_SIGNS {
                for host in HOSTS {
                    for tail in TAILS {
                        rejected_by_both(&format!("{scheme}{userinfo}{at}{host}{tail}"));
                        count += 1;
                    }
                }
            }
        }
    }
    // Without a scheme a path, port or bracketed host marks the word as an address.
    for userinfo in USERINFO {
        for at in AT_SIGNS {
            for host in HOSTS.iter().filter(|host| **host != "intranet") {
                for tail in TAILS.iter().filter(|tail| !tail.is_empty()) {
                    rejected_by_both(&format!("{userinfo}{at}{host}{tail}"));
                    count += 1;
                }
            }
        }
    }
    assert!(count > 3_000, "the generator covers the product: {count}");
}

const SECRET_NAMES: &[&str] = &[
    "token",
    "access_token",
    "Access-Token",
    "accessToken",
    "ACCESS_TOKEN",
    "api_key",
    "apiKey",
    "APIKey",
    "API.KEY",
    "x-api-key",
    "apikey",
    "client_secret",
    "private_token",
    "PRIVATE-TOKEN",
    "refresh_token",
    "password",
    "passwd",
    "pwd",
    "pass",
    "jwt",
    "session",
    "sig",
    "key",
    "auth",
    "signature",
    "X-Amz-Signature",
    "private_key",
    "access_key",
    "secret",
    "--token",
    "%74oken",
    "%2574oken",
    "t%6Fken",
    "%61pi%5Fkey",
];
const SEPARATORS: &[&str] = &["?", "&", ";", "/", "#", "?a=1&", "?tab=x;", "\\", "#top&"];
const BASES: &[&str] = &[
    "https://github.com/o/r/issues/6",
    "github.com/o/r/issues/6",
    "http://github.com",
    "",
];

#[test]
fn generated_secret_parameters_are_rejected_by_label_and_url() {
    let mut count = 0;
    for base in BASES {
        for separator in SEPARATORS {
            for name in SECRET_NAMES {
                for value in ["abc", "ghp_0123", "a%20b"] {
                    rejected_by_both(&format!("{base}{separator}{name}={value}"));
                    count += 1;
                }
            }
        }
    }
    assert!(count > 1_000, "the generator covers the product: {count}");
}

#[test]
fn generated_benign_links_are_accepted_by_label_and_url() {
    for path in [
        "/o/r/issues/6",
        "/ops/dpm/pull/7/files",
        "/group/sub/proj/-/merge_requests/3",
        "/o/r/discussions/9",
        "",
        "/",
    ] {
        for suffix in [
            "",
            "?tab=files",
            "#token-refresh",
            "#api-key-setup",
            "#issuecomment-123",
            "?tab=activity#top",
            "?page=2&tab=commits",
        ] {
            for scheme in ["https://", "http://", "HTTPS://"] {
                for host in [
                    "github.com",
                    "GitHub.com",
                    "github.com:443",
                    "github.com:0443",
                ] {
                    let url = format!("{scheme}{host}{path}{suffix}");
                    assert_eq!(check_text(&url), Ok(()), "label {url}");
                    assert_eq!(check_url(&url, &github()), Ok(()), "url {url}");
                }
            }
        }
    }
}
