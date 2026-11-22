use super::*;

#[test]
fn extracts_exact_editions_and_retains_artwork_and_credit_roles() {
    let detail = r#"
        <div class="video-detail"><span class="current-title">站点标题</span><span class="origin-title">原片名</span></div>
        <div class="movie-panel-info">
          <div class="panel-block first-block"><strong>番号:</strong><span class="value">ZZZ - 005</span></div>
          <div class="panel-block"><strong>評分:</strong><span class="value">4.25 分 (12 人)</span></div>
          <div class="panel-block"><strong>時長:</strong><span class="value">80 分鐘</span></div>
          <div class="panel-block"><strong>簡介:</strong><span class="value">这是一段中文简介。</span></div>
          <div class="panel-block"><strong>演員:</strong><span class="value">
            <a href="/actors/1" class="cast actor-female">演员甲</a>
            <a href="/actors/2" class="actor-male">演员乙</a>
            <a href="/actors/3">演员丙</a></span></div>
          <div class="panel-block"><strong>導演:</strong><span class="value"><a href="/directors/1">导演甲</a></span></div>
        </div>
        <div class="column-video-cover"><img class="video-cover" src="/cover.jpg"></div>
        <div class="preview-images"><a class="tile-item" href="/still.jpg" data-fancybox="yes"><img src="/thumb.jpg"></a></div>
    "#;
    let url = Url::parse("https://javdb.com/v/example").unwrap();
    let id = Identifier::parse("ZZZ-005").unwrap();
    let work = parse(detail, &url, &id).ok().unwrap();
    assert_eq!(work.fields["title_original"], "原片名");
    assert_eq!(work.data["chinese_metadata"]["title"], "站点标题");
    assert_eq!(
        work.data["chinese_metadata"]["description"],
        "这是一段中文简介。"
    );
    assert_eq!(work.fields["runtime_minutes"], 80);
    assert_eq!(
        work.fields["rating"],
        json!({"value":4.25,"count":12,"scale":5})
    );
    assert_eq!(work.fields["actresses"], json!(["演员甲"]));
    assert_eq!(work.fields["actors"], json!(["演员乙"]));
    assert_eq!(work.fields["other_cast"], json!(["演员丙"]));
    assert_eq!(work.fields["directors"], json!(["导演甲"]));
    assert_eq!(work.fields["cover_url"], "https://javdb.com/cover.jpg");
    assert_eq!(
        work.data["gallery"][0]["thumbnail_url"],
        "https://javdb.com/thumb.jpg"
    );
    assert!(matches!(
        parse(detail, &url, &Identifier::parse("ZZZ-006").unwrap()),
        Err(Failure::NotFound)
    ));
    assert!(matches!(
        parse("<title>服务暂不可用</title>", &url, &id),
        Err(Failure::Unavailable(_))
    ));
}
