use super::*;

const NAME: &str = "R18.dev";

pub(super) async fn lookup(client: &Client, code: &Identifier) -> Source {
    let endpoint = |kind: &str, value: &str| {
        Url::parse(&format!(
            "https://r18.dev/videos/vod/movies/detail/-/{kind}={value}/json"
        ))
        .unwrap()
    };
    let dvd_url = endpoint("dvd_id", &code.formatted);
    let dvd = fetch_json(client, &dvd_url).await;
    let content_id = match &dvd {
        Ok(data) => data["content_id"].as_str().filter(|value| {
            !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }),
        Err(_) => None,
    };
    let combined_url = endpoint("combined", content_id.unwrap_or(&code.compact));
    let combined = fetch_json(client, &combined_url).await.and_then(|data| {
        let matches = ["dvd_id", "content_id"]
            .into_iter()
            .any(|key| data[key].as_str().is_some_and(|value| code.matches(value)))
            || content_id.is_some_and(|id| data["content_id"] == id);
        matches.then_some(data).ok_or(Failure::NotFound)
    });

    let (details, fallback) = match (&combined, &dvd) {
        (Ok(details), Ok(fallback)) => (details, fallback),
        (Ok(details), Err(_)) => (details, &Value::Null),
        (Err(_), Ok(fallback)) if content_id.is_some() => (&Value::Null, fallback),
        (Err(error @ Failure::Unavailable(_)), _) => {
            return Source::failure(
                NAME,
                &combined_url,
                Failure::Unavailable(error.description().into()),
            );
        }
        (_, Err(error)) => return Source::failure(NAME, &dvd_url, error.clone()),
        _ => return Source::failure(NAME, &combined_url, Failure::format_changed()),
    };

    let fallback_names = |key: &str| names(fallback.get(key));
    let fallback_title = fallback["title"].clone();
    let fields = field_map([
        ("dvd_id", details["dvd_id"].clone()),
        (
            "content_id",
            value_or(&details["content_id"], &fallback["content_id"]),
        ),
        ("title_ja", details["title_ja"].clone()),
        ("title_en", value_or(&details["title_en"], &fallback_title)),
        (
            "release_date",
            value_or(&details["release_date"], &fallback["release_date"]),
        ),
        (
            "runtime_minutes",
            value_or(&details["runtime_mins"], &fallback["runtime_minutes"]),
        ),
        (
            "studio",
            value_or(&details["maker_name_en"], &fallback["maker"]["name"]),
        ),
        (
            "label",
            value_or(&details["label_name_en"], &fallback["label"]["name"]),
        ),
        (
            "series",
            value_or(&details["series_name_en"], &fallback["series"]["name"]),
        ),
        (
            "directors",
            value_or(
                &names(details.get("directors")),
                &json!(
                    fallback["director"]
                        .as_str()
                        .into_iter()
                        .collect::<Vec<_>>()
                ),
            ),
        ),
        (
            "actresses",
            value_or(
                &names(details.get("actresses")),
                &fallback_names("actresses"),
            ),
        ),
        ("actors", names(details.get("actors"))),
        (
            "genres",
            value_or(
                &names(details.get("categories")),
                &fallback_names("categories"),
            ),
        ),
        (
            "description",
            value_or(&details["comment_en"], &details["comment_ja"]),
        ),
        ("description_original", details["comment_ja"].clone()),
        (
            "cover_url",
            value_or(
                &details["jacket_full_url"],
                &fallback["images"]["jacket_image"],
            ),
        ),
        ("poster_url", details["jacket_thumb_url"].clone()),
        (
            "trailer_url",
            value_or(&details["sample_url"], &fallback["sample"]["high"]),
        ),
    ]);
    let has_details = combined.is_ok();
    let mut source = Source::found(
        NAME,
        if has_details { &combined_url } else { &dvd_url },
        fields,
        json!({"combined_json": details, "dvd_lookup_json": fallback}),
    );
    if let (Err(error), true) = (&combined, dvd.is_ok()) {
        source.status = "partial";
        source.note = Some(format!(
            "已取得番号查询资料。详细接口：{}",
            error.description()
        ));
    }
    source
}

fn value_or(preferred: &Value, fallback: &Value) -> Value {
    if preferred.is_null()
        || preferred.as_array().is_some_and(Vec::is_empty)
        || preferred
            .as_str()
            .is_some_and(|text| text.trim().is_empty())
    {
        fallback.clone()
    } else {
        preferred.clone()
    }
}

async fn fetch_json(client: &Client, url: &Url) -> Result<Value, Failure> {
    let body = request(client, url, "application/json, text/html").await?;
    let data = serde_json::from_str::<Value>(&body).map_err(|_| Failure::format_changed())?;
    if data.is_null() {
        return Err(Failure::NotFound);
    }
    if data["content_id"].as_str().is_none() {
        return Err(Failure::format_changed());
    }
    Ok(data)
}
