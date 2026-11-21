use super::*;

const NAME: &str = "JavDB";

pub(super) async fn lookup(client: &Client, code: &Identifier) -> Source {
    let mut search_url = Url::parse("https://javdb.com/search").unwrap();
    search_url
        .query_pairs_mut()
        .append_pair("q", &code.dvd_candidate())
        .append_pair("f", "all");
    let pages = match request(client, &search_url, "text/html")
        .await
        .and_then(|html| exact_matches(&html, &search_url, code))
    {
        Ok(pages) => pages,
        Err(error) => return Source::failure(NAME, &search_url, error),
    };
    let mut first_error = None;
    let mut sources = Vec::new();
    let details = futures_util::future::join_all(pages.iter().take(4).map(|url| async {
        request(client, url, "text/html")
            .await
            .and_then(|html| parse(&html, url, code))
    }))
    .await;
    for result in details {
        match result {
            Ok(source) => sources.push(source),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    if sources.is_empty() {
        return Source::failure(NAME, &pages[0], first_error.unwrap_or(Failure::NotFound));
    }

    let mut primary = sources.remove(0);
    primary.data["matching_pages"] = json!(pages.iter().map(Url::as_str).collect::<Vec<_>>());
    primary.data["alternate_editions"] = json!(sources);
    if first_error.is_some() || pages.len() > 4 {
        primary.status = "partial";
        primary.note = Some("已取得此番号的资料。部分版本尚未读取，可查看匹配的来源链接。".into());
    }
    primary
}

fn exact_matches(html: &str, search_url: &Url, code: &Identifier) -> Result<Vec<Url>, Failure> {
    let document = Html::parse_document(html);
    let list = Selector::parse(".movie-list").unwrap();
    let empty = Selector::parse(".empty-message").unwrap();
    if document.select(&list).next().is_none() && document.select(&empty).next().is_none() {
        return Err(Failure::format_changed());
    }
    let mut pages = Vec::new();
    for card in document.select(&Selector::parse(".movie-list a.box[href]").unwrap()) {
        let Some(id) = card
            .select(&Selector::parse(".video-title strong").unwrap())
            .next()
            .map(text)
        else {
            continue;
        };
        if !code.matches(&id) {
            continue;
        }
        if let Some(page) = card
            .attr("href")
            .and_then(|path| search_url.join(path).ok())
            .filter(|url| {
                url.scheme() == "https"
                    && url.host_str() == search_url.host_str()
                    && url.path().starts_with("/v/")
            })
            && !pages.contains(&page)
        {
            pages.push(page);
        }
    }
    if pages.is_empty() {
        Err(Failure::NotFound)
    } else {
        Ok(pages)
    }
}

fn parse(html: &str, url: &Url, code: &Identifier) -> Result<Source, Failure> {
    let document = Html::parse_document(html);
    let dvd_id = attribute(
        &document,
        ".movie-panel-info .first-block [data-clipboard-text]",
        "data-clipboard-text",
    )
    .or_else(|| {
        first_text(&document, ".movie-panel-info .first-block .value")
            .map(|value| value.replace(' ', ""))
    })
    .ok_or(Failure::format_changed())?;
    if !code.matches(&dvd_id) {
        return Err(Failure::NotFound);
    }
    let mut rows = Vec::new();
    for item in document.select(&Selector::parse(".movie-panel-info > .panel-block").unwrap()) {
        let Some(label) = item
            .select(&Selector::parse("strong").unwrap())
            .next()
            .map(text)
        else {
            continue;
        };
        let Some(value) = item.select(&Selector::parse(".value").unwrap()).next() else {
            continue;
        };
        rows.push(json!({
            "label": label.trim_end_matches([':', '：']),
            "text": text(value),
            "links": links(value, url),
        }));
    }
    let row = |labels: &[&str]| {
        rows.iter().find(|row| {
            row["label"]
                .as_str()
                .is_some_and(|label| labels.contains(&label))
        })
    };
    let value = |labels: &[&str]| {
        row(labels)
            .map(|row| row["text"].clone())
            .unwrap_or(Value::Null)
    };
    let row_links = |labels: &[&str]| {
        row(labels)
            .and_then(|row| row["links"].as_array())
            .cloned()
            .unwrap_or_default()
    };
    let current_title = first_text(&document, ".video-detail .current-title");
    let original_title = first_text(&document, ".video-detail .origin-title");
    let title = original_title.as_ref().or(current_title.as_ref());
    let image = |selector, name| {
        attribute(&document, selector, name).and_then(|value| web_url(url, &value))
    };
    let cover = image(".column-video-cover img.video-cover", "src");
    let gallery: Vec<_> = document
        .select(&Selector::parse(".preview-images a.tile-item[data-fancybox]").unwrap())
        .filter_map(|item| {
            Some(json!({
                "full_url": web_url(url, item.attr("href")?)?,
                "caption": item.attr("data-caption"),
                "thumbnail_url": item.select(&Selector::parse("img").unwrap()).next()
                    .and_then(|img| web_url(url, img.attr("src")?)),
            }))
        })
        .collect();
    let people = row_links(&["演員", "演员", "Actors", "Cast"]);
    let people_by_role = |class: &str| {
        people
            .iter()
            .filter(|link| {
                link["class"]
                    .as_str()
                    .unwrap_or_default()
                    .split_whitespace()
                    .any(|value| value == class)
            })
            .filter_map(|link| link["name"].as_str())
            .collect::<Vec<_>>()
    };
    let other_cast = people
        .iter()
        .filter(|link| {
            link["class"].as_str().is_none_or(|classes| {
                !classes
                    .split_whitespace()
                    .any(|class| ["actor-female", "actor-male"].contains(&class))
            })
        })
        .filter_map(|link| link["name"].as_str())
        .collect::<Vec<_>>();
    let rating = value(&["評分", "评分", "Rating"]);
    let rating_text = rating.as_str().unwrap_or_default();
    let mut score_parts = rating_text
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter(|value| !value.is_empty());
    let score = score_parts
        .next()
        .and_then(|value| value.parse::<f64>().ok());
    let count = score_parts
        .next()
        .and_then(|value| value.parse::<u64>().ok());
    let fields = field_map([
        ("dvd_id", json!(dvd_id)),
        ("title_original", json!(title)),
        (
            "title_site",
            json!(
                current_title
                    .as_ref()
                    .filter(|text| Some(*text) != original_title.as_ref())
            ),
        ),
        ("release_date", value(&["日期", "Date", "Release Date"])),
        (
            "runtime_minutes",
            json!(
                value(&["時長", "时长", "Duration"])
                    .as_str()
                    .and_then(number_in)
            ),
        ),
        ("studio", value(&["片商", "Maker", "Studio"])),
        ("label", value(&["發行", "发行", "Publisher", "Label"])),
        ("series", value(&["系列", "Series"])),
        (
            "directors",
            names(Some(&json!(row_links(&["導演", "导演", "Director"])))),
        ),
        ("actresses", json!(people_by_role("actor-female"))),
        ("actors", json!(people_by_role("actor-male"))),
        ("other_cast", json!(other_cast)),
        (
            "genres",
            names(Some(&json!(row_links(&["類別", "类别", "Tags", "Genres"])))),
        ),
        ("description", value(&["簡介", "简介", "Synopsis"])),
        (
            "rating",
            score
                .map(|value| json!({ "value": value, "scale": 5, "count": count }))
                .unwrap_or_default(),
        ),
        ("cover_url", json!(cover)),
    ]);
    Ok(Source::found(
        NAME,
        url,
        fields,
        json!({
            "rows": rows,
            "current_title": current_title,
            "original_title": original_title,
            "cover_url": cover,
            "gallery": gallery,
            "community_activity": first_text(&document, ".movie-panel-info > .panel-block > .has-text-grey"),
        }),
    ))
}

#[cfg(test)]
#[path = "../../tests/javdb.rs"]
mod detail_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_near_matches_and_cross_origin_targets() {
        let html = r#"<div class="movie-list">
            <a class="box" href="/v/wrong"><div class="video-title"><strong>ABC-1234</strong></div></a>
            <a class="box" href="//example.org/v/wrong"><div class="video-title"><strong>ABC-123</strong></div></a>
            <a class="box" href="/v/right"><div class="video-title"><strong>ABC-123</strong></div></a>
            <a class="box" href="/v/other"><div class="video-title"><strong>ABC-123</strong></div></a>
        </div>"#;
        let base = Url::parse("https://javdb.com/search").unwrap();
        let result = exact_matches(html, &base, &Identifier::parse("abc123").unwrap())
            .ok()
            .unwrap();
        assert_eq!(
            result.iter().map(Url::path).collect::<Vec<_>>(),
            ["/v/right", "/v/other"]
        );
    }
}
