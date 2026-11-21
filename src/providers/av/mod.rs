mod javdatabase;
mod javdb;
mod r18;

use reqwest::{Client, Url};
use scraper::{ElementRef, Html, Selector};
use serde_json::{Value, json};

use super::{Failure, field_map, request, text};
use crate::collect::{Identifier, Source};

pub(crate) async fn collect(client: &Client, code: &Identifier) -> Vec<Source> {
    // Product IDs often need R18's mapping before the other sites can match the DVD ID.
    let (r18, by_number) = if code.formatted.contains('-') {
        let (r18, javdatabase, javdb) = tokio::join!(
            r18::lookup(client, code),
            javdatabase::lookup(client, code),
            javdb::lookup(client, code),
        );
        (r18, Some((javdatabase, javdb)))
    } else {
        (r18::lookup(client, code).await, None)
    };

    let canonical = r18
        .fields
        .get("dvd_id")
        .and_then(Value::as_str)
        .and_then(|value| Identifier::parse(value).ok());
    let lookup_code = canonical.as_ref().unwrap_or(code);
    let (javdatabase, javdb) = match by_number {
        Some(results) if lookup_code.compact == code.compact => results,
        _ => tokio::join!(
            javdatabase::lookup(client, lookup_code),
            javdb::lookup(client, lookup_code)
        ),
    };
    vec![r18, javdatabase, javdb]
}

fn first_text(document: &Html, selector: &str) -> Option<String> {
    document
        .select(&Selector::parse(selector).unwrap())
        .next()
        .map(text)
}

fn web_url(base: &Url, value: &str) -> Option<String> {
    base.join(value)
        .ok()
        .filter(|url| {
            url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
        })
        .map(Into::into)
}

fn attribute(document: &Html, selector: &str, name: &str) -> Option<String> {
    document
        .select(&Selector::parse(selector).unwrap())
        .next()?
        .attr(name)
        .map(str::to_owned)
}

fn links(parent: ElementRef<'_>, base: &Url) -> Vec<Value> {
    parent
        .select(&Selector::parse("a[href]").unwrap())
        .filter_map(|link| {
            let name = text(link);
            (!name.is_empty()).then(|| {
                json!({
                    "name": name,
                    "url": web_url(base, link.attr("href").unwrap_or_default()),
                    "class": link.attr("class")
                })
            })
        })
        .collect()
}

fn names(links: Option<&Value>) -> Value {
    json!(
        links
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|link| link
                .get("name")
                .or_else(|| link.get("name_kanji"))
                .or_else(|| link.get("name_romaji"))
                .or_else(|| link.get("name_ja"))
                .or_else(|| link.get("name_en"))
                .and_then(Value::as_str))
            .collect::<Vec<_>>()
    )
}

fn number_in(text: &str) -> Option<u32> {
    text.split(|c: char| !c.is_ascii_digit())
        .find_map(|part| part.parse().ok())
}
