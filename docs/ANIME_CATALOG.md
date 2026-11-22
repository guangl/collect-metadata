# 动画番剧目录

目录以 [Bangumi Archive](https://github.com/bangumi/Archive) 的动画条目和章节为基础，收录 TV、WEB、OVA 及媒介未分类的动画条目。历年较早的条目保留为续作关系的前作，近 20 年季度查询使用滚动日期范围。未公开日期的条目保留在基础目录中，能找到首集日期时用它确定播出季度。仅有年份的条目保留年份精度。

[open-ani/bangumi-tmdb](https://github.com/open-ani/bangumi-tmdb) 补充 Bangumi 条目到 TMDB 作品、季、集的对应关系和覆盖状态。数据包含待复核与可能失效的目标。TMDB 的季集坐标可以覆盖官方多个季或一个季的部分篇章，参考目录保留原始目标范围和来源版本。确认本地编号时遵循[季集映射规范](EMBY_SEASON_MAPPING.md)。

- `collect_metadata_anime_quarters` 按首播季度列出番剧条目，区分公开集数、正片条目数、特别篇数和日期截至今日的正片条目数。后者用于跟踪排片，实际播出可能延期。
- `collect_metadata_anime_sequels` 按 Bangumi 前后作关系查询续作，显示下一条目的首播日期和集数。前后篇也可能形成续作关系。
- `collect_metadata_anime_tmdb_subjects` 和 `collect_metadata_anime_tmdb_episodes` 保留公开的跨站映射及来源版本。
- `collect_metadata_anime_verified_seasons` 保存已核对的 Emby 正式季，含首播日、整季集数和按播放顺序排列的 Bangumi 条目。启用 `EMBY_AUTO_SCRAPE_DB_SEASON_MAP=true` 后自动刮削优先读取该表。新登记的行下一次刮削即可用。缺少数据库映射行时可沿用现有 JSON 配置，再走普通匹配。

季度视图的行是 Bangumi 播出条目。制作方把上下篇归到同一正式季时，已核对的季表用一行保存整季总集数。

```sql
SELECT quarter_start, coalesce(title_zh, title) AS title, catalog_premiere,
       premiere_precision, expected_episodes, regular_episodes_listed
FROM collect_metadata_anime_quarters
WHERE quarter_start = DATE '2026-10-01'
ORDER BY quarter_date, title;
```

## 更新

运行 `python3 tools/anime-catalog/fetch_snapshots.py <cache>` 获取并校验发布数据，再运行 `python3 tools/anime-catalog/import_catalog.py <cache> | psql -X -v ON_ERROR_STOP=1`。设置 `pipefail`，并通过 PostgreSQL 连接环境将导入指向 Collect Metadata 数据库。更新使用单事务和稳定来源 ID，重跑同版本不会创建重复记录。初次迁移可以通过 `--verified-seasons <json>` 提供已核对映射。以后刷新不会覆盖已核对行。每次导入的 URL、版本和摘要保存在 `collect_metadata_anime_imports`。

Bangumi 源数据遵循 [Bangumi Archive 许可约定](https://bgm.tv/about/agreement)。open-ani 的映射遵循 [CC BY 4.0](https://github.com/open-ani/bangumi-tmdb/blob/main/LICENSE-DATA)。
