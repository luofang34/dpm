use super::*;

#[test]
fn credentials_of_every_known_shape_are_replaced() {
    for secret in [
        "sk-abcdef1234567890XYZ",
        "ghp_0123456789abcdefghij",
        "github_pat_11ABCDEFG0123456789",
        "xoxb-1234567890-abcdefgh",
        "AKIAIOSFODNN7EXAMPLE",
        "AIzaSyA-1234567890abcdefghijklmnop",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig12345",
        "0123456789abcdef0123456789abcdef",
        "QWxhZGRpbjpvcGVuIHNlc2FtZSBhbmQgbW9yZSBvcGFxdWUgYmFzZTY0",
    ] {
        let text = redact(&format!("see {secret} here"));
        assert_eq!(text, format!("see {REDACTED} here"), "{secret}");
    }
}

#[test]
fn named_secrets_lose_their_values_in_both_spellings_and_after_bearer() {
    assert_eq!(redact("password=hunter2 ok"), "password=<redacted> ok");
    assert_eq!(
        redact("api_key: abc123 next"),
        "api_key=<redacted> <redacted> next"
    );
    assert_eq!(
        redact("Authorization: Bearer abcdefghijkl"),
        "Authorization=<redacted> Bearer <redacted>"
    );
    assert_eq!(
        redact("send Bearer abcdefghijkl now"),
        "send Bearer <redacted> now"
    );
}

#[test]
fn home_directories_become_a_tilde_and_ordinary_words_are_left_alone() {
    assert_eq!(
        redact("read /Users/someone/work/file.txt"),
        "read ~/work/file.txt"
    );
    assert_eq!(redact("in /home/ci/x and /Users/someone"), "in ~/x and ~");
    assert_eq!(
        redact("The maintenance fixture says: ok"),
        "The maintenance fixture says: ok"
    );
}

#[test]
fn control_characters_and_newlines_become_single_spaces() {
    assert_eq!(redact("a\n\tb\u{7}c  d"), "a b c d");
}

#[test]
fn bounding_cuts_on_a_character_boundary_and_says_so() {
    assert_eq!(bounded("short", 10), "short");
    let cut = bounded("αβγδεζηθ", 9);
    assert!(cut.len() <= 9, "{cut}");
    assert!(cut.ends_with('…'));
    assert!(public_text(&"x ".repeat(500), 20).len() <= 20);
}

#[test]
fn only_identifier_shaped_text_is_an_identifier() {
    assert_eq!(
        identifier("toolu_01FqSueFShN5W78kwB6yyYJv").as_deref(),
        Some("toolu_01FqSueFShN5W78kwB6yyYJv")
    );
    assert!(identifier("e073a351-a251-4f5b-9ad4-2c717f993045").is_some());
    assert_eq!(identifier(""), None);
    assert_eq!(identifier("has space"), None);
    assert_eq!(identifier(&"a".repeat(101)), None);
}

#[test]
fn named_flags_and_authentication_schemes_lose_their_values() {
    for (text, expected) in [
        ("curl --token synthetic-value", "curl --token <redacted>"),
        (
            "app --password synthetic-value now",
            "app --password <redacted> now",
        ),
        (
            "app --api-key = synthetic-value",
            "app --api-key = <redacted>",
        ),
        ("app --token --verbose", "app --token --verbose"),
        (
            "Authorization: Basic c3ludGhldGljOnNlY3JldA==",
            "Authorization=<redacted> Basic <redacted>",
        ),
        (
            "-H \"Authorization:Basic c3ludGhldGlj\" next",
            "-H \"Authorization=<redacted> <redacted>\" next",
        ),
        ("a basic usage guide", "a basic usage guide"),
    ] {
        assert_eq!(redact(text), expected, "{text}");
    }
}

#[test]
fn url_userinfo_and_secret_query_values_are_removed_wherever_the_url_appears() {
    for (text, expected) in [
        (
            "failed URL https://tester:synthetic-value@example.test/x",
            "failed URL https://<redacted>@example.test/x",
        ),
        (
            "(see https://synthetic-value@example.test:8443/a?b=1),",
            "(see https://<redacted>@example.test:8443/a?b=1),",
        ),
        (
            "GET https://example.test/p?id=7&api_key=synthetic-value&x=1",
            "GET https://example.test/p?id=7&api_key=<redacted>&x=1",
        ),
        (
            "see https://example.test/tokens/1?x=2",
            "see https://example.test/tokens/1?x=2",
        ),
    ] {
        assert_eq!(redact(text), expected, "{text}");
    }
}
