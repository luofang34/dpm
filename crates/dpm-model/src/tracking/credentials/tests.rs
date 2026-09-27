use super::{check_text, check_url};

#[test]
fn labels_reject_credentials_in_urls_without_a_scheme_or_in_any_encoding() {
    for label in [
        "see github.com/x?token=abc",
        "tok@github.com/ops/dpm/issues/42",
        "(ghp_abc@github.com/ops/dpm)",
        "tok%40github.com/ops/dpm",
        "localhost:3000/x?key=abc",
        "<github.com/x?sig=abc>",
        "www.example.com/x#access_token=abc",
        "https://github.com/x?%74oken=x",
        "github.com/x?%2574oken=x",
        "github.com/x?%6Bey=1",
        "HTTPS://GITHUB.COM/X?ACCESS_TOKEN=1",
        "https://github.com/x?Api%5FKey=1",
        "https://github.com/x?x-api.key=1",
        "password=hunter2",
        "Deploy with --token=abc",
        "PRIVATE%2DTOKEN=abc",
    ] {
        assert!(check_text(label).is_err(), "accepted {label}");
    }
}

#[test]
fn ordinary_label_text_is_not_mistaken_for_a_credential() {
    for label in [
        "Contact user@example.com about #auth",
        "Fix issue #42 in ops/dpm",
        "Refactor auth/code key handling",
        "Token refresh, e.g./i.e. forms",
        "Why? Because x=1 and token= parsing",
        "https://github.com/ops/dpm/issues/42?tab=activity#top",
        "Review 1.5/2 of the plan (codeberg.org/ops/dpm)",
    ] {
        assert_eq!(check_text(label), Ok(()), "{label}");
    }
}

#[test]
fn urls_reject_encoded_and_case_varied_secret_parameters() {
    let instance = "github.com";
    for url in [
        "https://github.com/x?%74oken=x",
        "https://github.com/x?%2574oken=x",
        "https://github.com/x?API%2DKEY=1",
        "HTTPS://GITHUB.COM/x#Access_Token=1",
        "https://github.com/x?x.api.key=1",
        "https://tok%40x@github.com/x",
    ] {
        assert!(check_url(url, instance).is_err(), "accepted {url}");
    }
    assert_eq!(
        check_url("https://github.com/ops/dpm/pull/7?tab=files", instance),
        Ok(())
    );
}
