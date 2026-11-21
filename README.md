# Collect Metadata

[![CI](https://github.com/guangl/collect-metadata/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/guangl/collect-metadata/actions/workflows/ci.yml)
[![桌面版下载](https://img.shields.io/github/v/release/guangl/collect-metadata?label=桌面版)](https://github.com/guangl/collect-metadata/releases/latest)
[![Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

**给本地文件夹和 Emby 媒体库用的自托管元数据刮削工具。** 在一个工作台里预览作品、提交后台任务、查看结果并导出资料。工作台不移动或改写原视频；Emby 插件另有默认关闭的散文件整理。

[项目站](https://guangl.github.io/collect-metadata/) · [快速上手](#快速上手) · [桌面版](#tauri-桌面版) · [Emby 插件](#emby-入库自动刮削插件) · [贡献指南](CONTRIBUTING.md) · [安全报告](SECURITY.md)

- **导入**：自动读取 Emby 全部媒体库，也可扫描本地文件夹、服务端挂载目录或手动输入作品清单。
- **后台采集**：支持暂停、继续和重试；有多个匹配时可手动确认。任务与历史保存在 PostgreSQL。
- **查看与导出**：按作品类型或原始语言筛选历史，查看海报和来源字段，导出 JSON、图片或 Emby NFO。

## 快速上手

从 [GitHub Releases](https://github.com/guangl/collect-metadata/releases/latest) 下载桌面安装包，不需要 Docker 或 Linux：

| 系统 | 安装包 |
| --- | --- |
| macOS（Apple Silicon / ARM64） | 下载 `macos-arm64.dmg`，打开后将 App 拖入「应用程序」 |
| Windows（x64） | 下载 `windows-x64-setup.exe`，运行安装程序 |

macOS 版未经过 Apple 公证，Windows 安装器未使用商业证书签名。请从本仓库下载，并可用 Release 随附的 `SHA256SUMS` 校验。

打开应用后，选择本地媒体文件夹即可预览作品。提交后台批次和查看历史需要在「设置 → PostgreSQL」连接数据库，详见 [Tauri 桌面版](#tauri-桌面版)。[Docker Web 版](#docker-web-版) 是可选部署方式，不是桌面版的前置条件。

## 采集范围

| 来源 | 可获取的资料 |
| --- | --- |
| Wikidata | 电影和剧集片名、原始语言、演职员、片长、发行日期、类型、制作国家或地区及外部 ID。保存原始条目和可解析的关联名称。 |
| TVmaze | 剧集简介、平台、评分、演职员、各季和分集资料，包含来源提供的特别篇、别名与图片信息。 |
| TMDB | 电影和剧集的译名、原片名、原始语言、简介、演职员、评分、发行与分级、图片、预告、播放平台、外部 ID。剧集另含各季与分集，支持特别篇。随记录保留来源返回的 JSON。 |
| R18.dev | 日文和英文标题、商品 ID、发售日期、时长、演职员、制作商、厂牌、系列、分类、封面、剧照及预告链接。保存两个接口返回的原始 JSON。 |
| JAVDatabase | 发行资料、英文标题、站点简介、关联人物和分类链接、收藏数、封面和剧照。保存从影片详情页提取的元数据及站点字段。 |
| JavDB | 站点标题、原片名、社区评分和评分人数、演职员、发行与分类资料、剧照和观看统计。保存从影片详情页提取的元数据及站点字段。 |

AV 番号按 R18.dev、JAVDatabase、JavDB 的顺序逐字段取值。根据 Wikidata 提供的明确关联 ID，合并同一作品的 TMDB 或 TVmaze 资料。各字段注明来源，媒体资源以 URL 保存。JavDB 最多读取搜索页中的前四个精确匹配详情，匹配链接随结果保存。

普通电影和剧集先加载精简候选列表，选中作品后才补查详情，短期复用搜索已取得的原始条目。独立的站点请求和版本详情会并发执行，并限制同时进行的采集数量。普通电影和剧集的覆盖取决于公开目录与关联条目。[Wikidata](https://www.wikidata.org/wiki/Wikidata:Data_access) · [TVmaze API（CC BY-SA）](https://www.tvmaze.com/api#licensing)

借鉴了 Amane 按番号组织条目、逐字段择优和保留独立来源结果的做法。[Amane](https://github.com/sqzw-x/amane)

## 配置 TMDB

打开 Web 或桌面版顶部导航的「设置」，在「TMDB」中填写 TMDB 账户页面中的 **API Read Access Token** 或 **v3 API Key**。可以先点「测试连接」验证凭据。「保存配置」会再次验证，并立即应用到电影、剧集和后台批次。保存后可留空输入框，直接测试当前凭据。[TMDB API 认证](https://developer.themoviedb.org/docs/authentication-application)

Docker Web 将凭据保存在 `tmdb_config` 卷中。桌面版保存在与 `database-url` 同目录的 `tmdb-token` 文件。直接运行 Web 程序时，可用 `TMDB_CONFIG_FILE` 指定配置文件。未保存配置时，两端都可从 `TMDB_READ_ACCESS_TOKEN` 环境变量读取凭据。

启用后，后台批次可按片名或剧名匹配 TMDB。也可在工作台手动输入 `tmdb:496243` 这类明确 ID。TMDB 搜索和详情优先请求所选的首选元数据语言；「原始语言」筛选不受此设置影响。

启用 TMDB 时提交的电影与剧集条目由同样配置了令牌的后台进程领取。桌面端和 Web 共用 PostgreSQL 时，可分别为两个进程配置令牌。

## 元数据语言与翻译

在 Web 或桌面版的「设置 → 元数据语言」可多选最多 8 种目标语言。默认是简体中文（`zh-CN`）；清空后保存也会恢复默认。首选项用于默认显示、文件命名和 Emby NFO。详情可以切换其他目标语言；快照和作品 JSON 保留 `metadata_languages` 和 `localized_metadata`。这不会改动作品的原始语言、原始来源 JSON 或旧快照。

Emby 插件同步时优先使用 Emby 当前条目的有效元数据语言（包括媒体库和服务器的继承设置），而不是界面语言。Emby 未配置元数据语言时，使用 Collect Metadata Web 的目标语言列表。同步结果写在条目 JSON 的 `_collect_metadata` 中，`Name`、`Overview` 等原始字段不变；不会把译文写回 Emby，也不改动 Emby 的刮削器或首选语言。

有来源译文时优先使用。标题、简介、宣传语和分集文字缺少目标语言、但有其他语言原文时，可交给「元数据翻译」补齐。没配置翻译、接口不可用或译文为空时保留原文；切换其他语言时，无对应译文的字段继续显示首选资料或原文。

### 接入 NAS 的「元数据翻译」

现有 NAS 定时任务只写回简体中文，没有按请求翻译的 API。[`tools/metadata_translation_bridge.py`](tools/metadata_translation_bridge.py) 是可选适配器，在同一个 Sublarr Python 环境里复用 NAS 任务已使用的 `DeepSeekBackend` 与凭据。它不会启动或改动原定时任务。适配器会把待译元数据文本交给 DeepSeek；仅在允许外发这些文本时启用。它有独立的每日额度，默认 100 条文本（UTC），通过 `METADATA_TRANSLATION_DAILY_LIMIT` 调整。

1. 将适配器挂载或复制到 Sublarr 容器中，在该容器的应用工作目录启动。示例（在 NAS 上执行；先将单独生成的翻译令牌安全地设置为宿主环境变量）：

   ```sh
   docker cp tools/metadata_translation_bridge.py media-sublarr:/tmp/metadata_translation_bridge.py
   docker exec -d -e METADATA_TRANSLATION_TOKEN -e METADATA_TRANSLATION_BIND=0.0.0.0 media-sublarr \
     python3 -B -c 'import runpy; runpy.run_path("/tmp/metadata_translation_bridge.py", run_name="__main__")'
   ```

   `docker exec` 适合验证，容器重建后需重新启动；长期运行请用持久挂载和服务管理。端口 7100 只允许可信内部网络访问，不要发布到公网。桥接预算默认存于 `/config/collect-metadata-translation.db`。
2. 给 Collect Metadata 设置 `METADATA_TRANSLATION_URL`（完整地址，如同一 Docker 网络中的 `http://media-sublarr:7100/translate`）和同一个 `METADATA_TRANSLATION_TOKEN`，然后重新启动 Web/桌面进程。Docker Web 从 `.env` 取值，修改后需要重建应用容器；跨不可信网络使用 HTTPS。

也可接入其他兼容服务：接受 Bearer 令牌及 `POST {"target_language":"ja","texts":["Source text"]}`，返回 `{"translations":["翻訳文"]}`。译文数组顺序与输入一致，单项无法翻译可返回 `null`。

## 文件夹刮削

保存 Emby 连接后，打开刮削工作台会自动显示全部 Emby 媒体库。切换到「文件夹」可递归识别子目录和视频文件名，按作品分组预览。点击「开始刮削元数据」会创建后台任务，可在队列中查看进度。

| 文件路径 | 识别结果 |
| --- | --- |
| `电影/寄生虫 (2019)/寄生虫.2019.2160p.mkv` | 电影《寄生虫》，年份 2019 |
| `动画电影/[字幕组][你的名字。][2016][1080p].mkv` | 电影《你的名字。》，年份 2016 |
| `暗黑 (2017)/Season 01/S01E02.mkv` | 剧集《暗黑》，年份 2017 |
| `AV/SSIS-001.1080p.mp4` | 番号 `SSIS-001` |
| `电影/[tmdb-496243].mkv` | 使用指定的 TMDB ID |

扫描会去掉常见分辨率、编码和发行标签。同一部作品的多集或多个版本归到同一条，样片、花絮和非视频文件会跳过。同名作品按文件名或目录名里的年份匹配。如果结果有歧义，可在该条进度里选择作品。每次最多扫描 30,000 个文件、提交 2,000 部作品。

桌面版使用本地文件夹选择器，Web 只读取所选目录中的文件名和相对路径。预览无需 PostgreSQL；提交刮削后，系统才把识别结果、进度和关联文件路径存入 PostgreSQL。工作台的文件夹刮削不会移动、重命名或修改原视频。

### NAS / SMB / NFS 共享目录

应用读取**已经挂载的**网络共享，不直接登录 SMB 或 NFS 服务，也不保存网络磁盘账号和密码。

- **桌面版**：先在系统挂载共享，再用「选择媒体文件夹」选取。macOS 的 SMB 共享可在访达用「连接服务器」挂载；Windows 可在资源管理器映射网络驱动器。NFS 使用操作系统的 NFS 客户端挂载。选盘体验取决于系统是否能访问挂载目录。
- **Web 版**：可用浏览器选择客户端能访问的目录，也可在「文件夹 → 浏览服务器挂载目录」中选择服务端的共享。直接运行 Web 服务时，给 `SCRAPE_MEDIA_ROOT` 设置一个已挂载的绝对路径。Docker 部署时，先在宿主机挂载共享，在 `.env` 设置 `SCRAPE_MEDIA_MOUNT=/宿主机/共享目录`，再运行：

  ```sh
  docker compose -f compose.yaml -f compose.media.yaml up -d --build
  ```

  Docker 将该目录只读挂载到容器的 `/media-input`。容器用户 UID 65532 需要读取文件名和访问子目录的权限。只挂载准备刮削的目录，不要暴露 NAS 根目录；能访问此 Web 界面的用户能浏览该共享范围内的目录。服务端目录选择器不接受共享根目录之外的路径。

### 输出文件

在「设置 → 输出与命名」打开「将其他来源的刮削文件另存到输出目录」，选择输出路径、作品目录名、资料文件名前缀和资料类型。可以分别输出作品 JSON、原始来源 JSON、海报、背景图、剧照及分集截图。作品 JSON 可勾选简介、演职员、评分和季集。人物头像可按演员、声优、导演、编剧及其他幕后人员选择，是否有头像取决于来源。

设置随新的文件夹批次一起保存。后台在所选目录下为每批建立 `batch-编号/work-编号`。作品目录名和资料文件名前缀可使用 `{title}`（片名）、`{year}`（年份）、`{id}`（来源 ID）、`{type}`（作品类型）、`{row}`（条目编号）。自定义作品目录名会附加条目编号。资料文件名前缀应用于 JSON 和图片。`files.json` 记录文件、数据来源及对应的媒体文件，Emby NFO 沿用媒体文件名。每部作品最多写出 20 张剧照和 100 项人物头像，队列里会显示输出路径和写入反馈。已排队的输出任务由提交它的桌面应用或 Web 服务继续执行。

桌面版使用目录选择器。Docker Web 使用挂载到容器 `/output` 的 `scrape_output` 卷，可在页面填写其中的相对子目录。若要直接写入宿主机文件夹，在 `.env` 设置 `SCRAPE_OUTPUT_MOUNT=/宿主机/输出路径` 并重建应用容器。该目录须允许容器内的 UID 65532 写入。

### Emby 和家庭元数据翻译

在「设置 → 家庭 Emby」填写服务地址和 API 密钥，测试后启用「优先读取 Emby 媒体库」。文件夹刮削会按片名、年份或明确的 TMDB ID 查找已有作品。应用将作品、季集和来源记录保存到 PostgreSQL，可在历史里查看。

工作台按媒体库提供独立标签页，显示当前库的电影、剧集和私人影片。打开作品后可在「简介」「季与分集」「演职员」间切换；单集也有独立的简介和演职员。导演、声优、演员等按角色分组，头像通过 Collect Metadata 读取 Emby 已有图片；来源没有头像时只显示占位。点击「开始刮削元数据」会将所有库的电影、剧集和私人影片加入同一个后台批次，重复出现的 Emby 条目只提交一次。单独采集时，可在剧集详情点「刮削此剧集」提交整部剧集，或进入某一集点「刮削此分集」只采集该集。即使关闭「优先读取 Emby 媒体库」，工作台仍会自动读取已配置的 Emby 服务器。私人影片可直接使用 Emby 已有的标题、简介和图片，无需 TMDB ID。

在「设置 → 输出与命名 → Emby 媒体写回」添加路径映射。Emby 端目录填写服务器看到的路径，例如 `/media/TV`。桌面版的本地目录填写对应的可写绝对路径，例如 `/Volumes/TV`。多个映射匹配时使用最具体的目录。Docker Web 可用以下方式挂载可写媒体目录，本地目录填写挂载中的相对路径，留空表示挂载根目录。挂载须允许容器用户 UID 65532 写入。

```sh
EMBY_WRITEBACK_MOUNT=/宿主机/媒体目录 docker compose -f compose.yaml -f compose.emby-writeback.yaml up -d --build
```

剧集根目录写入 `tvshow.nfo` 和可用封面、背景图。电影和分集的 NFO 用真实视频文件名，例如 `第 1 集.mkv` 对应 `第 1 集.nfo`，分集缩略图放在同一目录。Emby 图片文件名遵循[电影](https://emby.media/support/articles/Movie-Naming.html#video-images)和[分集](https://emby.media/support/articles/TV-Naming.html#episode-images)的约定。写回会替换同名 NFO 或图片。媒体未挂载、路径未映射或写入失败时，后台队列会显示原因，成功获取的元数据仍保存在 PostgreSQL。

家庭「元数据翻译」的旧定时任务在 NAS 处理作品简介和分集标题，并写回 Emby。采集会读取 Emby 已有资料；如需额外的目标语言，可配置上面的翻译适配器。文件夹刮削在 Emby 未匹配到作品时继续查询常规来源。

批次另存时，勾选「Emby NFO」可生成电影、剧集和可匹配分集的 XML。NFO 可写入排序名、原名、简介及原文、发行日期、分级、评分及投票数、演职员、系列、标签、厂牌、图片与预告链接。能获得哪些字段取决于来源。

批次另存的「附加媒体」可分别勾选预告片、主题视频、主题音乐和花絮，支持直链下载。每类每部作品最多下载 10 个文件，单文件上限 256 MiB。TMDB 返回的 YouTube 或 Vimeo 播放页会写入 `media-links.json`，主要预告链接也会写入 NFO。输出反馈会显示哪些已选类型没有文件或仅有链接。

导入 Emby 时，把 `trailers/`、`backdrops/`（主题视频）、`theme-music/` 和 `extras/` 随作品放在同一目录。参见 [Emby 预告片命名](https://emby.media/support/articles/Trailers.html)和 [主题媒体命名](https://emby.media/support/articles/Theme-Songs-Videos.html)。

## Emby 入库自动刮削插件

[`emby-plugin/`](emby-plugin/) 是独立安装在 Emby Server 里的原生插件。Emby 扫描发现新电影、剧集、季、分集或私人影片后，插件会通过 Emby 的后台队列刷新该条目的元数据和图片。给已有剧集添加新分集时，只对新入库的条目提交刷新。插件不会对已有条目做批量回填，也不会请求「替换所有元数据」或「替换所有图片」；已锁定的条目会被跳过。

使用 [Emby Server 4.9.1.90 的 SDK](https://www.nuget.org/packages/MediaBrowser.Server.Core/4.9.1.90) 构建，已在 4.10.2.0 Docker 镜像中验证同步。其他版本请先验证兼容性。构建需要 .NET 8 SDK 或更新版本：

```sh
dotnet build emby-plugin/CollectMetadata.Emby.csproj -c Release
```

1. 只把 `emby-plugin/bin/Release/netstandard2.0/CollectMetadata.Emby.dll` 复制到 **Emby 服务端数据目录**的 `plugins/`（官方 Docker 镜像中通常是 `/config/plugins/`）。确保 Emby 进程能读取 DLL，然后重启 Emby；不要复制任何 SDK DLL。
2. 在 Emby 控制台的「媒体库」为电影和剧集分别启用所需的元数据下载器、图片下载器和首选语言。插件沿用这些设置，不需要 Collect Metadata Web、PostgreSQL 或本应用里配置的 TMDB 密钥。如果未开启实时监控，文件要等计划扫描或手动扫描才会入库。
3. 在测试库放入一部新电影或一个新分集，等 Emby 扫描和后台刷新完成。在 Emby 日志中可看到 `Collect Metadata - Auto Scrape` 记录的 `Requested metadata refresh for …`（表示提交刷新，不代表匹配成功）。元数据结果取决于文件命名、媒体库设置和下载器的连通性。

### 自动整理媒体库根目录（可选）

升级 Emby 插件后，可以直接把电影或有明确季集号的分集放进对应的 Emby 媒体库根目录。插件默认每 2 分钟检查一次，只处理连续两次观察到大小和修改时间未变的视频；大文件建议先复制到库外，完成后再移入，避免复制暂停时被误判为已完成。

| 放入库根目录 | 整理结果（相对于库根目录） |
| --- | --- |
| 电影库：`片名 (2025).mkv` | `片名 (2025)/片名 (2025).mkv` |
| 剧集库：`剧名 S01E02.mkv` | `剧名/Season 1/剧名 S01E02.mkv` |

已经放在子目录内的视频不动；电影发现已有同名作品目录时跳过，不合并。剧集会复用唯一匹配的剧集目录（如 `剧名 (2024) [tmdbid=…]`）和已有 `Season 1` / `Season 01`；遇到同名剧集时，在分集文件名里加年份以区分。根目录中同名 NFO、字幕和作品图片会随文件移动，不覆盖目标文件。无法识别季集号、目录有歧义、或仅在混合媒体库中的文件会留在原处。整理后插件通知 Emby 重新识别，Emby 的 NFO 保存器和图片下载器将资料写在最终作品目录；能否取得资料仍取决于命名、来源和网络。

1. 在 Emby 对应媒体库设置中启用 **NFO 元数据保存器**和**将图片保存到媒体文件夹**，并启用所需的元数据和图片下载器。不满足前两项时整理任务会跳过该库，不会移动视频。
2. 在 **Emby 管理后台 → 插件 → Collect Metadata · 自动整理** 选择允许整理的媒体库路径。Collect Metadata 的「设置 → 家庭 Emby」也提供入口。填 Emby 中实际登记的库根（如 `/media/.private-video`），不要填上层 `/media` 或容器外的 NAS 路径。未选择路径不会整理。以前配置了 `EMBY_ORGANIZE_ROOTS` 的部署会继续使用该变量，页面会标明路径只读；要在页面修改路径，移除该环境变量即可。整理开关仍可关闭任务。隐藏的库根可以明确启用，但库内隐藏视频仍跳过。
3. 给 Emby **仅开放所选库目录的写入权限**。如果父挂载 `/media` 是只读的，可单独给需要整理的库增加可写子挂载，不必把所有库改成可写。挂载变更需要重建容器；网页保存的开关和模板从下一次任务生效，无需重启。在「计划任务」里运行 **Organize loose library videos** 或调整检查频率。

**命名设置：** 可分别配置电影目录、剧集目录、季目录、电影文件名和分集文件名。默认文件模板为 `{original}`，不改名。电影目录例如 `{title}{year_suffix}{tmdb_suffix}`，季目录例如 `Season {season:00}`，分集文件名例如 `{title} - {episode_code}`。年份和 TMDB ID 从原名提取；没有这些信息时，`{year_suffix}` 和 `{tmdb_suffix}` 不生成空括号。扩展名自动保留，选中随视频移动的 NFO、字幕、图片会同步更改文件名前缀。分集重命名必须保留季集号；如果模板会丢掉合集范围，该文件沿用原名。页面可为电影和分集分别启用重命名，选择是否从新文件名移除年份、TMDB 标记（不影响目录模板），并设置 60–3600 秒的稳定等待时间。命名编辑器提供预设、变量插入和可编辑的文件名样例。可直接粘贴 ani-rss 常用写法 `${title} S${seasonFormat}E${episodeFormat}`；另支持 `${subgroup}`、`${resolution}` 和 `${itemTitle}`，其中字幕组与分辨率来自原文件名。未识别到的可选信息不会联网补齐。预览不扫描磁盘；磁盘上的已有文件和目录仍按不覆盖规则处理。

建议先启用一个测试库。启用后也会处理该库根目录里已有的散文件；Emby 可能把移动过的文件重新识别为新条目。如果没及时看到新条目，可手动扫描该媒体库。日志里的 `Organized video` 表示文件已移动，不代表外站匹配成功。

可用 `dotnet run --project emby-plugin-tests/OrganizerChecks.csproj` 在临时目录验证整理规则，无需连接 NAS 或 Emby。

### 自动同步到 PostgreSQL

插件只连接 Collect Metadata Web 服务，**不需要 PostgreSQL 地址、账号或密码**。配置方法：

1. 在 Collect Metadata 的 `.env` 中设置 `EMBY_SYNC_TOKEN`（建议用 `openssl rand -hex 32` 生成），然后 `docker compose up -d --build`。不要提交包含令牌的文件。
2. 在 Emby 的 **Collect Metadata · 自动整理 → 同步到 Collect Metadata** 填写 Emby 能访问的服务地址和相同的同步令牌。两个容器在同一个 Docker 网络时，可用 `http://app:8080`（Compose 默认服务名）；不要用 `localhost` 指向另一个容器。如果经过 NAS 发布端口，需将 `APP_HOST` 绑定到 Emby 能访问的主机接口。跨不可信网络请使用 HTTPS。已有的 `COLLECT_METADATA_URL` 和 `EMBY_SYNC_TOKEN` 环境变量仍优先，页面会把对应字段标为只读。
3. 初次配置后，在 Emby 控制台「计划任务」中运行 **Sync library to Collect Metadata**。之后默认每 30 分钟同步一次；也可在该任务里调整频率。服务不可用时，下一次运行会重新推送，不需要手工补单。

任务同步已有和新增的电影、剧集、季、分集及私人影片（不含虚拟占位项和花絮）；Emby 内更新的简介、标题等会在下次同步时更新。PostgreSQL 的 `collect_metadata_emby_items` 按 `(server_id, item_id)` 保存最新的元数据 JSON，图片只保存 Emby 的引用，不传图片或视频文件。该表独立于手动采集的历史快照。重复同步不新增行；从 Emby 删除条目不会自动删库。

```sql
SELECT document->>'Type' AS media_type, count(*)
FROM collect_metadata_emby_items
GROUP BY 1;
```

未配置同步时，插件仍自动刮削，但不会向服务发送数据。要完全停用插件，从 `plugins/` 删除 `CollectMetadata.Emby.dll` 后重启 Emby。

## 手动批量采集

在「刮削工作台」下展开「手动输入作品」，每行输入一个片名、剧名或番号。没有前缀的行使用表单里的默认类型，可用以下格式混合输入。

```text
电影：寄生虫
剧集：Dark
有码番号：ABC-123
无码番号：010124-123
素人番号：300MIUM-001
FC2番号：FC2-PPV-1234567
欧美番号：Brazzers.24.01.31
国产番号：MD-123
动漫里番：RJ123456
mywife：Mywife-0123
```

这些前缀和「番号：」都会将条目提交为 AV 类型，不需要为每类建单独的批次。文件夹扫描也可识别上述示例格式。常见的不同写法（如 `FC2_1234567` 和 `FC2-PPV-1234567`）会归到同一条作品。番号能识别不代表现有来源必定收录：欧美日期式番号目前识别常见厂牌，国产和动漫里番没有专用来源；未收录或未精确匹配时，不用相似结果代替。

每批最多 100 条，同时处理 3 条。唯一精确匹配会自动采集。同名或相近候选会进入「待选择」，可按年份和原始语言确认。等待选择的条目会让出并发位置。失败的条目可单独重试，已有结果可展开或导出 JSON。

后台服务会把批次、逐条阶段、候选和结果保存到 PostgreSQL。提交后可以关闭页面，重新打开「刮削工作台」即可在「后台任务」中选择原批次。完成的资料与任务状态在同一事务保存。

- 「暂停批次」停止处理未完成条目。「继续批次」恢复排队，并恢复原有待选候选。
- 「取消批次」永久停止该批次，已完成的结果仍可查看和导出。
- 「重试全部失败条目」重新排队所有标记为「失败」的条目。批次处于暂停状态时，重试条目会等待手动继续。

服务重启后，暂停和取消状态继续生效。后台只接手运行中批次的任务，原先正在处理的条目会在执行租约到期后重新领取，租约最长 15 秒。

同一个 PostgreSQL 数据库最多同时处理 3 条。连接数据库的桌面端关闭窗口时会保留托盘进程，可从托盘重新打开窗口。退出整个桌面进程后，独立运行的 Web 服务仍可处理同一数据库里的任务。

## 代码目录

| 路径 | 职责 |
| --- | --- |
| `src/bin/` | Tauri 桌面端和 Axum Web 端入口 |
| `src/collect/` | 采集流程、番号识别和元数据模型 |
| `src/batch/` | 批次操作和后台并发工作进程 |
| `src/providers/av/` | AV 番号来源 |
| `src/providers/media/` | 电影和剧集来源 |
| `src/storage/` | PostgreSQL 快照、历史查询和持久化任务队列 |
| `ui/src/batch/` | 批量采集界面与任务进度 |

## 测试

GitHub CI 在推送 `main` 或提交 PR 时运行前端测试与构建，以及 Rust 的桌面和 Web 测试，不需要私有凭据。`npm run test:coverage` 检查 TypeScript 源文件的行覆盖率，门槛为 85%。

Rust 的完整覆盖率检查还包含 PostgreSQL 集成测试及实际来源采集。运行前设置 `COLLECT_METADATA_TEST_POSTGRES`（连接账户需有建 schema 权限）和 `TMDB_READ_ACCESS_TOKEN`，并确保能连接测试使用的公开来源。

```sh
npm run build
cargo llvm-cov clean --workspace
EMBY_SYNC_TOKEN=fixture-key cargo llvm-cov --locked --all-features --summary-only --fail-under-lines 85 -- --include-ignored --test-threads=1
```

## Docker Web 版

从 [`.env.example`](.env.example) 复制本地 `.env` 并设置 `POSTGRES_PASSWORD`。以下两种部署方式都会启动 Web 应用和 PostgreSQL，并把数据库数据保存在 `postgres_data` 卷中。

**使用预构建镜像（Compose v2.24.4+）：** [GitHub Container Registry](https://github.com/users/guangl/packages/container/package/collect-metadata) 镜像供可选的 Docker 部署使用，按需手动构建，不随桌面 Release 更新。如需固定版本，在 `.env` 中把 `COLLECT_METADATA_IMAGE` 设置为 Packages 中列出的标签或镜像摘要。

```sh
docker compose -f compose.yaml -f compose.ghcr.yaml pull
docker compose -f compose.yaml -f compose.ghcr.yaml up -d
```

如果拉取时返回 `denied`，请检查镜像标签是否已发布，以及 GHCR 包的可见性；私有包需要先用有读取权限的账号登录 `ghcr.io`。也可以改用下面的方式从源码构建。

**从源码构建：**

```sh
docker compose up -d --build
```

浏览器打开 [http://127.0.0.1:8080](http://127.0.0.1:8080)。在 `.env` 中用 `APP_HOST` 和 `APP_PORT` 调整宿主机监听地址和端口。

直接运行 Web 服务器时，通过 `DATABASE_URL` 或标准 `PGHOST`、`PGPORT`、`PGDATABASE`、`PGUSER`、`PGPASSWORD` 连接已有 PostgreSQL。应用启动时会创建 `collect_metadata_snapshots` 表。每次有结果的采集写入一条 JSONB 快照，重新采集会新增记录。不要将应用或数据库端口直接暴露在公网，详见 [安全说明](SECURITY.md)。

```sh
npm ci
npm run build
cargo run --locked --no-default-features --features web --bin collect-metadata-web
```

## Tauri 桌面版

先准备 Rust、Node.js 24 或更新版本，以及对应平台的 Tauri 前置依赖。macOS 需要 Xcode 命令行工具，Windows 需要 Visual Studio C++ Build Tools 和 WebView2。[Tauri 前置依赖](https://v2.tauri.app/start/prerequisites/)

```sh
npm ci
npm run tauri -- dev
```

在对应平台打包，可在 `target/<目标平台>/release/bundle/` 找到产物。

```sh
# macOS（Apple Silicon）
npm run tauri -- build --target aarch64-apple-darwin --bundles dmg

# Windows（x64）
npm run tauri -- build --target x86_64-pc-windows-msvc --bundles nsis
```

桌面版无需连接数据库即可预览文件夹；提交后台批次和查看采集历史则需要 PostgreSQL。在「设置」的「PostgreSQL」中粘贴连接地址，点击「测试连接并保存」。也可在启动前设置 `DATABASE_URL`，或在下列路径写入一行连接串。环境变量优先。Web 和桌面版都可以从「设置」测试当前数据库，Web 的连接地址由服务端环境变量提供。

如需桌面版和 Docker Web 版共用本机数据库，可以加上桌面用 Compose 配置。它仅在本机回环地址开放 PostgreSQL 端口。

```sh
docker compose -f compose.yaml -f compose.desktop.yaml up -d --build
```

桌面端连接 `127.0.0.1:15432` 的 `collect_metadata` 数据库，用户名为 `collect_metadata`，密码取 `.env` 中的 `POSTGRES_PASSWORD`。可用 `POSTGRES_LOCAL_PORT` 调整端口。

| 系统 | `database-url` 路径 |
| --- | --- |
| macOS | `~/Library/Application Support/com.collectmetadata.desktop/database-url` |
| Windows | `%APPDATA%\com.collectmetadata.desktop\database-url` |

## 许可证

本项目代码采用 [Apache License 2.0](LICENSE)。第三方元数据、图片和商标不因本项目的许可证而获得授权，使用时应遵守各来源的条款。
