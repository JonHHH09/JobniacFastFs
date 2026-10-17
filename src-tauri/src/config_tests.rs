//! Guards the window security configuration: the production CSP stays strict and the main window
//! has core defaults only.

use std::collections::BTreeMap;

const CONFIG: &str = include_str!("../tauri.conf.json");
const CAPABILITY: &str = include_str!("../capabilities/default.json");

/// Split a CSP string into directive name to source list.
fn directives(csp: &str) -> BTreeMap<String, Vec<String>> {
    csp.split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut tokens = part.split_whitespace().map(str::to_owned);
            let name = tokens.next().unwrap_or_default();
            (name, tokens.collect())
        })
        .collect()
}

fn csp_of(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .pointer("/app/security")
        .and_then(|security| security.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

#[test]
fn production_csp_is_strict() -> Result<(), serde_json::Error> {
    let config: serde_json::Value = serde_json::from_str(CONFIG)?;
    let csp = csp_of(&config, "csp").unwrap_or_default();
    let policy = directives(&csp);

    assert_eq!(policy.get("default-src"), Some(&vec!["'self'".to_owned()]));
    assert_eq!(policy.get("script-src"), Some(&vec!["'self'".to_owned()]));
    assert_eq!(policy.get("style-src"), Some(&vec!["'self'".to_owned()]));
    // Images may also come from the page-image scheme, on its platform forms only.
    assert_eq!(
        policy.get("img-src"),
        Some(&vec![
            "'self'".to_owned(),
            "http://jobniac-page.localhost".to_owned(),
            "jobniac-page://localhost".to_owned(),
        ])
    );
    assert!(
        !csp.contains("unsafe-inline"),
        "production CSP allows inline code: {csp}"
    );
    assert!(
        !csp.contains("data:"),
        "production CSP allows data URIs: {csp}"
    );
    // Only the local IPC origin and the page-image scheme may name a source outside the app.
    for source in policy.values().flatten() {
        assert!(
            !source.starts_with("http")
                || source == "http://ipc.localhost"
                || source == "http://jobniac-page.localhost",
            "remote origin in production CSP: {source}"
        );
    }
    Ok(())
}

#[test]
fn development_csp_only_adds_what_vite_needs() -> Result<(), serde_json::Error> {
    let config: serde_json::Value = serde_json::from_str(CONFIG)?;
    let dev_csp = csp_of(&config, "devCsp").unwrap_or_default();
    let policy = directives(&dev_csp);

    assert_eq!(
        policy.get("style-src"),
        Some(&vec!["'self'".to_owned(), "'unsafe-inline'".to_owned()])
    );
    assert_eq!(policy.get("script-src"), Some(&vec!["'self'".to_owned()]));
    assert!(
        !dev_csp.contains("data:"),
        "development CSP allows data URIs: {dev_csp}"
    );
    Ok(())
}

#[test]
fn main_window_capability_grants_core_defaults_only() -> Result<(), serde_json::Error> {
    let capability: serde_json::Value = serde_json::from_str(CAPABILITY)?;
    assert_eq!(
        capability.get("windows"),
        Some(&serde_json::json!(["main"]))
    );
    assert_eq!(
        capability.get("permissions"),
        Some(&serde_json::json!(["core:default"]))
    );
    Ok(())
}
