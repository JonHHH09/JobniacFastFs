//! Host name normalization against the recorded IDNA 2003 results.
//!
//! The recorded results normalize non-ASCII names with the IDNA 2003 codec; this crate follows UTS 46
//! through the `idna` crate (strict profile). ASCII names must agree exactly. Every non-ASCII input
//! whose result differs from the recorded one is listed in [`DEVIATIONS`] with both results, and the
//! test fails if the set of differences changes in either direction.

use jobniac_core::outbound::normalize_hostname;
use serde_json::Value;

const GOLDEN: &str = include_str!("golden/outbound-policy.json");

/// One recorded difference: the input, the recorded result and the Rust result (as text).
struct Deviation {
    host: &'static str,
    recorded: &'static str,
    rust: &'static str,
}

/// Recorded differences between the recorded IDNA 2003 codec and the UTS 46 implementation.
///
/// Two rules explain all 34 rows of the golden sample:
/// - `ß` (U+00DF): IDNA 2003 maps it to `ss` (transitional); UTS 46 (non-transitional) keeps it and
///   encodes it as punycode.
/// - A Unicode label that starts or ends with `-` (or has `--` in positions 3 and 4) is refused by
///   UTS 46 CheckHyphens; IDNA 2003 has no such check and encodes it.
const DEVIATIONS: &[Deviation] = &[
    Deviation {
        host: "straße.de",
        recorded: "ok:strasse.de",
        rust: "ok:xn--strae-oqa.de",
    },
    Deviation {
        host: "ß.de",
        recorded: "ok:ss.de",
        rust: "ok:xn--zca.de",
    },
    Deviation {
        host: "ac0ßba.xn--p1ai",
        recorded: "ok:ac0ssba.xn--p1ai",
        rust: "ok:xn--ac0ba-nqa.xn--p1ai",
    },
    Deviation {
        host: "例--例00X例例ßé0.de",
        recorded: "ok:xn----00xss0-h1a6042rcada.de",
        rust: "ok:xn----00x0-fta4fr846gcada.de",
    },
    Deviation {
        host: "XébX-.de",
        recorded: "ok:xn--xbx--bpa.de",
        rust: "refused:hostname",
    },
    Deviation {
        host: "Yßca.io",
        recorded: "ok:yssca.io",
        rust: "ok:xn--yca-5ka.io",
    },
    Deviation {
        host: "ßa0aaé0例éßZXbéa-10b-é例ZXY--.io",
        recorded: "ok:xn--ssa0aa0sszxba-10b-zxy---gccbfg04747coa.io",
        rust: "refused:hostname",
    },
    Deviation {
        host: "例b0.ZéZß-b.X1aaY.xn--p1ai",
        recorded: "ok:xn--b0-pt3c.xn--zzss-b-bva.x1aay.xn--p1ai",
        rust: "ok:xn--b0-pt3c.xn--zz-b-wna2d.x1aay.xn--p1ai",
    },
    Deviation {
        host: "YXéYc9ß例Z-.xn--p1ai",
        recorded: "ok:xn--yxyc9ssz--c4a9452t.xn--p1ai",
        rust: "refused:hostname",
    },
    Deviation {
        host: "0.0éXé9--Z99.ßacé-.io",
        recorded: "ok:0.xn--0x9--z99-b1ab.xn--ssac--esa.io",
        rust: "refused:hostname",
    },
    Deviation {
        host: "ZcY90aß9b9-ßZb119例éX例.de",
        recorded: "ok:xn--zcy90ass9b9-sszb119x-t2b37935bca.de",
        rust: "ok:xn--zcy90a9b9-zb119x-llbe9qp895mca.de",
    },
    Deviation {
        host: "9XXß例Y-ß19.bßc.c例c-.com",
        recorded: "ok:xn--9xxssy-ss19-4h2r.bssc.xn--cc--vc0e.com",
        rust: "refused:hostname",
    },
    Deviation {
        host: "éa9ßaXc.com",
        recorded: "ok:xn--a9ssaxc-9xa.com",
        rust: "ok:xn--a9axc-mqa1e.com",
    },
    Deviation {
        host: "9.0bca-Ycß例Zcc.例b9ßé-99-X.xn--p1ai",
        recorded: "ok:9.xn--0bca-ycsszcc-e08s.xn--b9ss-99-x-e4a9352t.xn--p1ai",
        rust: "ok:9.xn--0bca-yczcc-c4a4972v.xn--b9-99-x-1va4gq871h.xn--p1ai",
    },
    Deviation {
        host: "bXßX.xn--p1ai",
        recorded: "ok:bxssx.xn--p1ai",
        rust: "ok:xn--bxx-6ka.xn--p1ai",
    },
    Deviation {
        host: "ß1c0baßa99Xb例c.de",
        recorded: "ok:xn--ss1c0bassa99xbc-yk8x.de",
        rust: "ok:xn--1c0baa99xbc-u6af9703z.de",
    },
    Deviation {
        host: "aß-0é.0-cßé.xn--p1ai",
        recorded: "ok:xn--ass-0-fsa.xn--0-css-fsa.xn--p1ai",
        rust: "ok:xn--a-0-5ka6c.xn--0-c-7ka4c.xn--p1ai",
    },
    Deviation {
        host: "b-1Z-é11ßXZ.io",
        recorded: "ok:xn--b-1z-11ssxz-fbb.io",
        rust: "ok:xn--b-1z-11xz-m1a1i.io",
    },
    Deviation {
        host: "ß.Yéß.bb.éb.io",
        recorded: "ok:ss.xn--yss-bma.bb.xn--b-9fa.io",
        rust: "ok:xn--zca.xn--y-qfa3a.bb.xn--b-9fa.io",
    },
    Deviation {
        host: "例bßbé.xn--p1ai",
        recorded: "ok:xn--bssb-epa9881j.xn--p1ai",
        rust: "ok:xn--bb-gia5b2103e.xn--p1ai",
    },
    Deviation {
        host: "1例9.ßabb9cß.bbac例-1.io",
        recorded: "ok:xn--19-qt3c.ssabb9css.xn--bbac-1-ew9i.io",
        rust: "ok:xn--19-qt3c.xn--abb9c-kqaf.xn--bbac-1-ew9i.io",
    },
    Deviation {
        host: "1X1.9Y.cc例1YZaa1ßa.io",
        recorded: "ok:1x1.9y.xn--cc1yzaa1ssa-1h2r.io",
        rust: "ok:1x1.9y.xn--cc1yzaa1a-n1a7462t.io",
    },
    Deviation {
        host: "-Zé.90例Yé.ßßaa1例X1X--.écbé-.com",
        recorded: "ok:xn---z-cja.xn--90y-dma2671h.xn--ssssaa1x1x---c08s.xn--cb--9lac.com",
        rust: "refused:hostname",
    },
    Deviation {
        host: "例ß1aY例99ZY1.Z1bc1éZßc-X.1Zc9.de",
        recorded: "ok:xn--ss1ay99zy1-uy5pfa.xn--z1bc1zssc-x-fbb.1zc9.de",
        rust: "ok:xn--1ay99zy1-pya3152rea.xn--z1bc1zc-x-l1a2i.1zc9.de",
    },
    Deviation {
        host: "Zß0.com",
        recorded: "ok:zss0.com",
        rust: "ok:xn--z0-gia.com",
    },
    Deviation {
        host: "例éZ-1ßY1Z.de",
        recorded: "ok:xn--z-1ssy1z-90a3142r.de",
        rust: "ok:xn--z-1y1z-dta0f9771h.de",
    },
    Deviation {
        host: "bc99例é-.de",
        recorded: "ok:xn--bc99--esa3202l.de",
        rust: "refused:hostname",
    },
    Deviation {
        host: "00ß-bé例-例ZX.xn--p1ai",
        recorded: "ok:xn--00ss-b-zx-g4a4452tba.xn--p1ai",
        rust: "ok:xn--00-b-zx-1va6gu871hba.xn--p1ai",
    },
    Deviation {
        host: "例例Y例acß-.xn--p1ai",
        recorded: "ok:xn--yacss--9v9iab.xn--p1ai",
        rust: "refused:hostname",
    },
    Deviation {
        host: "Y9X9ßééß.io",
        recorded: "ok:xn--y9x9ssss-g1aa.io",
        rust: "ok:xn--y9x9-ynaa3ea.io",
    },
    Deviation {
        host: "béß.XXZcc例X-cY1cß.io",
        recorded: "ok:xn--bss-bma.xn--xxzccx-cy1css-fj5u.io",
        rust: "ok:xn--b-qfa3a.xn--xxzccx-cy1c-56a7292x.io",
    },
    Deviation {
        host: "9a.0例例-éß.01a9a9a1bY.com",
        recorded: "ok:9a.xn--0-ss-cpa2981ja.01a9a9a1by.com",
        rust: "ok:9a.xn--0--hia3b2458ea.01a9a9a1by.com",
    },
    Deviation {
        host: "Y-c1例-cb1-.com",
        recorded: "ok:xn--y-c1-cb1--tg9n.com",
        rust: "refused:hostname",
    },
    Deviation {
        host: "b1Z.1éa例Yßé9ab-例Z例0éZ.例0ß-1例.de",
        recorded: "ok:b1z.xn--1ayss9ab-z0z-bebeg38133aiab.xn--0ss-1-4d3hf.de",
        rust: "ok:b1z.xn--1ay9ab-z0z-73a1jdg45031ahab.xn--0-1-5ka2181hea.de",
    },
];

fn outcome_text(result: Result<String, jobniac_core::outbound::OutboundPolicyError>) -> String {
    match result {
        Ok(name) => format!("ok:{name}"),
        Err(error) => format!("refused:{}", error.reason()),
    }
}

#[test]
fn ascii_hostnames_match_golden_exactly() {
    let document: Value = match serde_json::from_str(GOLDEN) {
        Ok(value) => value,
        Err(error) => panic!("outbound-policy.json does not parse: {error}"),
    };
    let rows = document["hostnames"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(rows.len() > 300, "only {} hostname rows", rows.len());
    let mut mismatches = Vec::new();
    for row in &rows {
        let host = row["host"].as_str().unwrap_or_default();
        if !host.is_ascii() {
            continue;
        }
        let recorded = match (row["ok"].as_str(), row["refused"].as_str()) {
            (Some(name), _) => format!("ok:{name}"),
            (_, Some(reason)) => format!("refused:{reason}"),
            _ => panic!("row without result: {row}"),
        };
        let rust = outcome_text(normalize_hostname(host));
        if recorded != rust {
            mismatches.push(format!("{host:?}: recorded {recorded}, rust {rust}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "ASCII mismatches:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn expected_differences_are_only_the_recorded_idna_rules() {
    let document: Value = match serde_json::from_str(GOLDEN) {
        Ok(value) => value,
        Err(error) => panic!("outbound-policy.json does not parse: {error}"),
    };
    let rows = document["hostnames"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut found: Vec<(String, String, String)> = Vec::new();
    for row in &rows {
        let host = row["host"].as_str().unwrap_or_default();
        if host.is_ascii() {
            continue;
        }
        let recorded = match (row["ok"].as_str(), row["refused"].as_str()) {
            (Some(name), _) => format!("ok:{name}"),
            (_, Some(reason)) => format!("refused:{reason}"),
            _ => panic!("row without result: {row}"),
        };
        let rust = outcome_text(normalize_hostname(host));
        if recorded != rust {
            found.push((host.to_owned(), recorded, rust));
        }
    }
    let expected: Vec<(String, String, String)> = DEVIATIONS
        .iter()
        .map(|item| {
            (
                item.host.to_owned(),
                item.recorded.to_owned(),
                item.rust.to_owned(),
            )
        })
        .collect();
    let describe: Vec<String> = found
        .iter()
        .map(|(host, recorded, rust)| format!("{host:?} => recorded {recorded}, rust {rust}"))
        .collect();
    assert_eq!(
        found,
        expected,
        "recorded differences changed; found:\n{}",
        describe.join("\n")
    );
}
