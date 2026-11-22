# 更新日志

## v0.12.1 · 2026-11-22

- **Emby 自动刮削**可根据 PostgreSQL 中已核对的季集映射，为已有标题、简介和缩略图的分集补写缺少的 Bangumi 分集 ID。保留已有 TVDB 等来源 ID、元数据和旁挂文件。

[与 v0.12.0 的完整差异](https://github.com/guangl/collect-metadata/compare/v0.12.0...v0.12.1)

## v0.12.0 · 2026-11-22

- **Emby 自动刮削**通过插件接收入库事件，补齐可核对的简介、季集名、海报和旁挂资料。保留已有资料，归档持续失败和已无法查到的入库事件。
- **动画目录**将番剧首播日期、正片集数、续作关系及 Bangumi 与 TMDB 的季集对应导入 PostgreSQL。已核对的 Emby 季映射支持跨上下篇；归档完整时只请求目标分集并校验线上编号，缺资料时读取实时季表。
- **后台任务**移至独立页面，提交后可直达对应批次。采集历史支持清空，Emby 海报详情增加来源链接和更多图片类型。
- **私人影片**改进番号命名、片商信息和中文简介补全。Web 与桌面端统一常见错误码，并提供日志与排查入口。

[与 v0.11.0 的完整差异](https://github.com/guangl/collect-metadata/compare/v0.11.0...v0.12.0)

## v0.11.0 · 2026-11-22

- **Emby 媒体库**支持按片名、原片名或年份搜索，筛选缺海报或简介的作品，勾选后批量采集。缺失字段可从公开目录补齐。
- **AI 刮削**可用 OpenAI 兼容模型辅助判断有歧义的电影和剧集候选，低置信度的匹配交给用户确认。元数据继续从公开来源或 Emby 采集。
- **纠正匹配**可在原任务条目重采电影、剧集或 Emby 分集，保留媒体文件关联，并记住已成功纠正的匹配。
- **MCP** 支持查询作品候选和 Emby 媒体库、提交批次、查看进度、读取结果和纠正匹配。
- **Emby 写回**支持预览、确认、替换前备份和逐文件重试。采集结果与写回状态分开保存。
- **发布**提供 macOS Apple Silicon、Windows x64 安装包、Emby 插件和 Docker Web 镜像 `ghcr.io/guangl/collect-metadata:0.11.0`。镜像支持 amd64 和 arm64。

[v0.11.0 Release](https://github.com/guangl/collect-metadata/releases/tag/v0.11.0) · [与 v0.10.0 的完整差异](https://github.com/guangl/collect-metadata/compare/v0.10.0...v0.11.0)

## v0.10.0 · 2026-11-22

- 接入 Bangumi，支持以条目 ID 或片名查询动画、电影和剧集。根据 Wikidata 提供的关联 ID 补齐 TMDB 和 TVmaze 资料。
- 扩展 Emby 媒体库工作台，可按全库、单库、单部作品或分集提交采集。丰富 NFO 字段，支持附加媒体导出。
- 改进家庭元数据翻译适配器，保留来自 Emby 的简介，并为适配器分配独立的用量数据库。

[v0.10.0 Release](https://github.com/guangl/collect-metadata/releases/tag/v0.10.0) · [与 v0.9.0 的完整差异](https://github.com/guangl/collect-metadata/compare/v0.9.0...v0.10.0)

## v0.9.0 · 2026-11-21

- 提供 macOS Apple Silicon 和 Windows x64 桌面安装包。可预览本地媒体文件夹，连接 PostgreSQL 后可提交批次和查看历史。

[v0.9.0 Release](https://github.com/guangl/collect-metadata/releases/tag/v0.9.0)
