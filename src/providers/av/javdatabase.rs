use super::*;

const NAME: &str = "JAVDatabase";

pub(super) async fn lookup(client: &Client, code: &Identifier) -> Source {
    let slug = code.dvd_candidate().to_ascii_lowercase();
    let url = Url::parse(&format!("https://www.javdatabase.com/movies/{slug}/")).unwrap();
    match request(client, &url, "text/html")
        .await
        .and_then(|html| parse(&html, &url, code))
    {
        Ok(source) => source,
        Err(error) => Source::failure(NAME, &url, error),
    }
}

fn parse(html: &str, url: &Url, code: &Identifier) -> Result<Source, Failure> {
    let document = Html::parse_document(html);
    let mut rows = Vec::new();
    for item in document.select(&Selector::parse(".movietable p.mb-1").unwrap()) {
        let Some(label) = item.select(&Selector::parse("b").unwrap()).next().map(text) else {
            continue;
        };
        let value = text(item);
        let value = value.strip_prefix(&label).unwrap_or(&value).trim();
        rows.push(json!({
            "label": label.trim_end_matches(':'),
            "text": value,
            "links": links(item, url),
        }));
    }
    let row = |label: &str| rows.iter().find(|row| row["label"] == label);
    let value = |label: &str| {
        row(label)
            .map(|row| row["text"].clone())
            .unwrap_or(Value::Null)
    };
    let ids = [value("DVD ID"), value("Content ID")];
    if !ids
        .iter()
        .any(|id| id.as_str().is_some_and(|id| code.matches(id)))
    {
        return Err(if rows.is_empty() {
            Failure::format_changed()
        } else {
            Failure::NotFound
        });
    }

    let take_url = |selector, name| {
        attribute(&document, selector, name).and_then(|value| web_url(url, &value))
    };
    let cover = take_url("#poster-container img", "src")
        .or_else(|| take_url("meta[property='og:image']", "content"));
    let poster = take_url("#thumbnailContainer img", "src");
    let trailer = take_url("#jav-player source", "src");
    let gallery: Vec<_> = document
        .select(&Selector::parse(".image-gallery-section [data-image-src]").unwrap())
        .filter_map(|item| {
            Some(json!({
                "full_url": web_url(url, item.attr("data-image-src")?)?,
                "thumbnail_url": item.select(&Selector::parse("img").unwrap()).next()
                    .and_then(|img| web_url(url, img.attr("src")?)),
            }))
        })
        .collect();
    let description = document
        .select(&Selector::parse(".movietable h4.subhead").unwrap())
        .find(|node| text(*node).starts_with("About "))
        .and_then(|heading| heading.parent())
        .and_then(ElementRef::wrap)
        .map(|parent| {
            parent
                .children()
                .filter_map(|node| node.value().as_text())
                .flat_map(|value| value.split_whitespace())
                .collect::<Vec<_>>()
                .join(" ")
        });
    let rating = first_text(&document, ".wp-postratings-text");
    let favorite_count = attribute(
        &document,
        ".movietable [data-favoritecount]",
        "data-favoritecount",
    )
    .and_then(|value| value.parse::<u64>().ok());
    let fields = field_map([
        ("dvd_id", ids[0].clone()),
        ("content_id", ids[1].clone()),
        ("title_en", value("Title")),
        ("release_date", value("Release Date")),
        (
            "runtime_minutes",
            json!(value("Runtime").as_str().and_then(number_in)),
        ),
        ("studio", value("Studio")),
        ("label", value("Label")),
        ("series", value("JAV Series")),
        (
            "directors",
            names(row("Director").and_then(|row| row.get("links"))),
        ),
        (
            "actresses",
            names(row("Idol(s)/Actress(es)").and_then(|row| row.get("links"))),
        ),
        (
            "genres",
            names(row("Genre(s)").and_then(|row| row.get("links"))),
        ),
        ("description", json!(description)),
        ("cover_url", json!(cover)),
        ("poster_url", json!(poster)),
        ("trailer_url", json!(trailer)),
    ]);
    Ok(Source::found(
        NAME,
        url,
        fields,
        json!({
            "rows": rows,
            "page_title": first_text(&document, ".entry-header h1"),
            "site_description": description,
            "updated_at": attribute(&document, "meta[property='og:updated_time']", "content"),
            "favorite_count": favorite_count,
            "rating_text": rating,
            "cover_url": cover,
            "poster_url": poster,
            "trailer_url": trailer,
            "gallery": gallery,
        }),
    ))
}
