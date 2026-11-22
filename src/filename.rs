// SPDX-License-Identifier: AGPL-3.0-only
use anyhow::{Context, Result, ensure};
use percent_encoding::percent_decode_str;

/// Portable upload basename, never a path. Do not silently repair or transliterate.
pub fn validate(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 255
            && name == name.trim()
            && !name.starts_with(['.', '-'])
            && !name.ends_with('.')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b" ._+-~:^".contains(&b)),
        "Invalid upload filename {name:?}: expected a portable ASCII basename without path separators, percent escapes or surrounding whitespace"
    );
    Ok(())
}

/// Only call on a URI component (or RFC 5987 value), never a literal asset name.
pub fn decode_component(raw: &str) -> Result<String> {
    let name = percent_decode_str(raw)
        .decode_utf8()
        .context("Invalid UTF-8 in encoded filename")?;
    validate(&name)?;
    Ok(name.into_owned())
}

pub fn url_basename(url: &str) -> Result<String> {
    ensure!(
        !url.chars().any(|c| c.is_control() || c == '\\'),
        "Control character or backslash in package URL"
    );
    let url = reqwest::Url::parse(url).context("Invalid package URL")?;
    decode_component(
        url.path_segments()
            .and_then(|mut p| p.next_back())
            .unwrap_or(""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_and_literal_names_have_distinct_encoding_rules() {
        let name =
            url_basename("https://host/p/tool%20x_2%3A1%7Erc1+2-amd64.deb?q=/other#f").unwrap();
        assert_eq!(name, "tool x_2:1~rc1+2-amd64.deb");
        for raw in ["%2520.deb", "%2Fother", "foo%5cbar", "%00bad", "%FF", "%zz"] {
            assert!(decode_component(raw).is_err());
        }
        for raw in [
            "literal%20name.deb",
            "../pkg",
            "a\u{fffd}.deb",
            "a\u{202e}.deb",
            " x.deb",
        ] {
            assert!(validate(raw).is_err());
        }
    }
}
