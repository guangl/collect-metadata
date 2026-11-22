# 更新日志

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
