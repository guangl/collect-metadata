# Collect Metadata

[![CI](https://github.com/guangl/collect-metadata/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/guangl/collect-metadata/actions/workflows/ci.yml)
[![桌面版下载](https://img.shields.io/github/v/release/guangl/collect-metadata?label=桌面版)](https://github.com/guangl/collect-metadata/releases/latest)
[![Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

**给本地文件夹和 Emby 媒体库用的自托管元数据刮削工具。** 在刮削工作台预览作品并提交采集，在「后台任务」查看进度、结果并导出资料。工作台不移动或改写原视频；Emby 插件另有默认关闭的散文件整理。

[项目站](https://guangl.github.io/collect-metadata/) · [快速上手](#快速上手) · [桌面版](#tauri-桌面版) · [Emby 插件](#emby-入库自动刮削插件) · [更新日志](CHANGELOG.md) · [贡献指南](.github/CONTRIBUTING.md) · [安全报告](.github/SECURITY.md) · [日志与排查](docs/TROUBLESHOOTING.md)

- **导入**：自动读取 Emby 全部媒体库，也可扫描本地文件夹、服务端挂载目录或手动输入作品清单。
- **后台采集**：支持暂停、继续和重试；有多个匹配时可手动确认。任务与历史保存在 PostgreSQL。
- **查看与导出**：按作品类型或原始语言筛选历史，查看海报和来源字段，导出 JSON、图片或 Emby NFO。

## 快速上手

从 [GitHub Releases](https://github.com/guangl/collect-metadata/releases/latest) 下载桌面安装包，不需要 Docker 或 Linux：

| 系统 | 安装包 |
| --- | --- |
| macOS（Apple Silicon / ARM64） | 下载 `macos-arm64.dmg`，打开后将 App 拖入「应用程序」 |
| Windows（x64） | 下载 `windows-x64-setup.exe`，运行安装程序 |

macOS 版未经过 Apple 公证，Windows 安装器未使用商业证书签名。请从本仓库下载，并可用 GitHub Release 资产旁显示的 SHA-256 摘要校验。

打开应用后，可以直接选择本地媒体文件夹刮削。使用持久化后台任务和采集历史时，在「设置 → PostgreSQL」连接数据库，详见 [Tauri 桌面版](#tauri-桌面版)。同一版本也提供 [Docker Web 镜像](#docker-web-版) 和 [Emby 插件](#emby-入库自动刮削插件)。已有部署请先查看 [从 v0.9.0 升级](#从-v090-升级)。

## 采集范围

| 来源 | 可获取的资料 |
| --- | --- |
| Wikidata | 电影和剧集片名、原始语言、演职员、片长、发行日期、类型、制作国家或地区及外部 ID。保存原始条目和可解析的关联名称。 |
| Bangumi | 动画、电影和剧集的译名、简介、评分、封面、主创、角色与配音、正片分集及特别篇。 |
| TVmaze | 剧集简介、平台、评分、演职员、各季和分集资料，包含来源提供的特别篇、别名与图片信息。 |
| TMDB | 电影和剧集的译名、原片名、原始语言、简介、演职员、评分、发行与分级、图片、预告、播放平台、外部 ID。剧集另含各季与分集，支持特别篇。随记录保留来源返回的 JSON。 |
| R18.dev | 日文和英文标题、商品 ID、发售日期、时长、演职员、制作商、厂牌、系列、分类、封面、剧照及预告链接。保存两个接口返回的原始 JSON。 |
| JAVDatabase | 发行资料、英文标题、站点简介、关联人物和分类链接、收藏数、封面和剧照。保存从影片详情页提取的元数据及站点字段。 |
| JavDB | 站点标题、原片名、社区评分和评分人数、演职员、发行与分类资料、剧照和观看统计。保存从影片详情页提取的元数据及站点字段。 |

AV 番号按 R18.dev、JAVDatabase、JavDB 的顺序逐字段取值。根据 Wikidata 提供的明确关联 ID，合并同一作品的 Bangumi、TMDB 和 TVmaze 资料。单独选择 Bangumi 条目也会查找这些关联。各字段注明来源，媒体资源以 URL 保存。JavDB 最多读取搜索页中的前四个精确匹配详情，匹配链接随结果保存。

普通电影和剧集先加载精简候选列表，选中作品后才补查详情，短期复用搜索已取得的原始条目。独立的站点请求和版本详情会并发执行，并限制同时进行的采集数量。普通电影和剧集的覆盖取决于公开目录与关联条目。[Wikidata](https://www.wikidata.org/wiki/Wikidata:Data_access) · [TVmaze API（CC BY-SA）](https://www.tvmaze.com/api#licensing)

借鉴了 Amane 按番号组织条目、逐字段择优和保留独立来源结果的做法。[Amane](https://github.com/sqzw-x/amane)

## 使用 Bangumi

选择电影或剧集后可输入片名，也可以直接使用条目 ID，例如 `bangumi:160209`（电影《你的名字。》）或 `bangumi:253`（剧集《星际牛仔》）。公开条目无需凭据。如需使用访问令牌，在 Web 或桌面版打开「设置 → Bangumi」，填入 Access Token，测试后保存即可生效。Docker Web 将令牌保存在 `tmdb_config` 卷，桌面版保存在配置目录的 `bangumi-token` 文件。还没有保存令牌时，可以通过 Docker 的 `.env` 或进程环境设置 `BANGUMI_ACCESS_TOKEN`。[Bangumi 公开 API](https://bangumi.github.io/api/)

Bangumi 分集按条目收录。一个条目可能只覆盖一季或一部分，导出时使用该批文件标明的单一季号。

## 配置 TMDB

打开 Web 或桌面版顶部导航的「设置」，在「TMDB」中填写 TMDB 账户页面中的 **API Read Access Token** 或 **v3 API Key**。可以先点「测试连接」验证凭据。「保存配置」会再次验证，并立即应用到电影、剧集和后台批次。保存后可留空输入框，直接测试当前凭据。[TMDB API 认证](https://developer.themoviedb.org/docs/authentication-application)

Docker Web 将凭据保存在 `tmdb_config` 卷中。桌面版保存在与 `database-url` 同目录的 `tmdb-token` 文件。直接运行 Web 程序时，可用 `TMDB_CONFIG_FILE` 指定配置文件。未保存配置时，两端都可从 `TMDB_READ_ACCESS_TOKEN` 环境变量读取凭据。

启用后，后台批次可按片名或剧名匹配 TMDB。也可在工作台手动输入 `tmdb:496243` 这类明确 ID。TMDB 搜索和详情优先请求所选的首选元数据语言；「原始语言」筛选不受此设置影响。

启用 TMDB 时提交的电影与剧集条目由同样配置了令牌的后台进程领取。桌面端和 Web 共用 PostgreSQL 时，可分别为两个进程配置令牌。

## 元数据语言与翻译

在 Web 或桌面版的「设置 → 元数据语言」可多选最多 8 种目标语言。默认是简体中文（`zh-CN`）；清空后保存也会恢复默认。首选项用于默认显示、文件命名和 Emby NFO。详情可以切换其他目标语言；快照和作品 JSON 保留 `metadata_languages` 和 `localized_metadata`。这不会改动作品的原始语言、原始来源 JSON 或旧快照。

Emby 插件同步时优先使用 Emby 当前条目的有效元数据语言（包括媒体库和服务器的继承设置），而不是界面语言。Emby 未配置元数据语言时，使用 Collect Metadata Web 的目标语言列表。同步结果写在条目 JSON 的 `_collect_metadata` 中，`Name`、`Overview` 等原始字段不变；不会把译文写回 Emby，也不改动 Emby 的刮削器或首选语言。

有来源译文时优先使用。分集详情会读取首选语言的简介。首选简体中文时，有关联的 Bangumi 条目可以补充能确认是同一集的中文简介。标题、简介、宣传语和分集文字缺少目标语言、但有其他语言原文时，可交给「元数据翻译」补齐。没配置翻译、接口不可用或译文为空时保留原文；切换其他语言时，无对应译文的字段继续显示首选资料或原文。

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
| `星际牛仔 [bangumi-253]/Season 01/S01E02.mkv` | 使用指定的 Bangumi 条目 ID |

扫描会去掉常见分辨率、编码和发行标签。同一部作品的多集或多个版本归到同一条，样片、花絮和非视频文件会跳过。同名作品按文件名或目录名里的年份匹配。如果结果有歧义，可在该条进度里选择作品。每次最多扫描 30,000 个文件、提交 2,000 部作品。

桌面版使用本地文件夹选择器，Web 只读取所选目录中的文件名和相对路径。预览无需 PostgreSQL。桌面版的数据库不可用时，刮削在当前窗口逐部完成，可查看和导出结果。启用了「输出与命名」的自动输出时，也会写出所选资料。Web 的文件夹刮削使用 PostgreSQL 后台任务。工作台的文件夹刮削不会移动、重命名或修改原视频。

### NAS / SMB / NFS 共享目录

应用读取**已经挂载的**网络共享，不直接登录 SMB 或 NFS 服务，也不保存网络磁盘账号和密码。

- **桌面版**：先在系统挂载共享，再用「选择媒体文件夹」选取。macOS 的 SMB 共享可在访达用「连接服务器」挂载；Windows 可在资源管理器映射网络驱动器。NFS 使用操作系统的 NFS 客户端挂载。选盘体验取决于系统是否能访问挂载目录。
- **Web 版**：可用浏览器选择客户端能访问的目录，也可在「文件夹 → 浏览服务器挂载目录」中选择服务端的共享。直接运行 Web 服务时，给 `SCRAPE_MEDIA_ROOT` 设置一个已挂载的绝对路径。Docker 部署时，先在宿主机挂载共享，在 `.env` 设置 `SCRAPE_MEDIA_MOUNT=/宿主机/共享目录`，再运行：

  ```sh
  docker compose -f compose.yaml -f deploy/compose.media.yaml up -d --build
  ```

  Docker 将该目录只读挂载到容器的 `/media-input`。容器用户 UID 65532 需要读取文件名和访问子目录的权限。只挂载准备刮削的目录，不要暴露 NAS 根目录；能访问此 Web 界面的用户能浏览该共享范围内的目录。服务端目录选择器不接受共享根目录之外的路径。

### 输出文件

在「设置 → 输出与命名」打开「将其他来源的刮削文件另存到输出目录」，选择输出路径、作品目录名、资料文件名前缀和资料类型。可以分别输出作品 JSON、原始来源 JSON、海报、背景图、剧照及分集截图。作品 JSON 可勾选简介、演职员、评分和季集。人物头像可按演员、声优、导演、编剧及其他幕后人员选择，是否有头像取决于来源。

设置随新的文件夹批次一起保存。后台在所选目录下为每批建立 `batch-编号/work-编号`。作品目录名和资料文件名前缀可使用 `{title}`（片名）、`{year}`（年份）、`{id}`（来源 ID）、`{type}`（作品类型）、`{row}`（条目编号）。自定义作品目录名会附加条目编号。资料文件名前缀应用于 JSON 和图片。`files.json` 记录文件、数据来源及对应的媒体文件，Emby NFO 沿用媒体文件名。每部作品最多写出 20 张剧照和 100 项人物头像，队列里会显示输出路径和写入反馈。已排队的输出任务由提交它的桌面应用或 Web 服务继续执行。

桌面版使用目录选择器。Docker Web 使用挂载到容器 `/output` 的 `scrape_output` 卷，可在页面填写其中的相对子目录。若要直接写入宿主机文件夹，在 `.env` 设置 `SCRAPE_OUTPUT_MOUNT=/宿主机/输出路径` 并重建应用容器。该目录须允许容器内的 UID 65532 写入。

### Emby 和家庭元数据翻译

在「设置 → 家庭 Emby」填写服务地址和 API 密钥，测试后启用「优先读取 Emby 媒体库」。文件夹刮削会按片名、年份或明确的 TMDB ID 查找已有作品。应用将作品、季集和来源记录保存到 PostgreSQL，可在历史里查看。

工作台按媒体库提供独立标签页，显示当前库的电影、剧集和私人影片。打开作品后可在「简介」「季与分集」「演职员」间切换；单集也有独立的简介和演职员。导演、声优、演员等按角色分组，头像通过 Collect Metadata 读取 Emby 已有图片；来源没有头像时只显示占位。点击「刮削全部媒体库」并确认后，会将所有库的电影、剧集和私人影片加入同一个后台批次，重复出现的 Emby 条目只提交一次。要单独处理动画电影、私人影片等媒体库，切换到相应标签页，点击「刮削此媒体库」。作品和分集详情中的刮削按钮只提交当前影片、剧集或分集。即使关闭「优先读取 Emby 媒体库」，工作台仍会自动读取已配置的 Emby 服务器。私人影片可直接使用 Emby 已有的标题、简介和图片，无需 TMDB ID。

媒体库内可以按片名、原片名或年份搜索，也可筛出缺海报或作品简介的条目。「分集缺少简介」会找出包含缺简介分集的剧集，打开后直接定位到相应分集。点击「选择」进入多选模式，勾选作品或点「选择当前结果」，再点「刮削所选」，每批最多 500 部。搜索和筛选变化后仍保留已勾选的作品。对缺资料的电影和剧集，后台尝试从公开目录补齐，优先沿用 Emby 已有的资料和图片。私人影片继续读取 Emby 资料。数据库不可用时，作品、分集、当前库和多选可以直接向 Emby 提交刮削，由 Emby 使用媒体库配置的下载器处理。

刮削当前媒体库、单部作品、分集或所选作品时，可在确认弹窗中选择「Emby 覆盖刮削」。确认后，Emby 服务器调用该媒体库启用的元数据下载器，重新获取资料并替换已有元数据。Emby 接收请求后会自行执行刮削，进度和结果可在 Emby 查看。勾选「同时替换已有图片」可一起更新图片。对媒体库或剧集操作时会递归刷新，对影片或分集只刷新当前条目。提交后可在 Emby 查看结果，完成后可点工作台的「重新读取媒体库」查看。

如果需要记录 IMDb 评分票数，可在应用配置目录放置 `imdb-ratings.json`，并重新启动应用。文件以 IMDb 作品 ID 为键，每条记录包含 `value`（10 分制 IMDb 评分）和 `count`（IMDb 票数），两者取自同一份 [IMDb 评分数据](https://data.imdb.com/non-commercial-datasets/)。作品详情和新采集记录会显示单独的 IMDb 评分与票数，导出的 NFO 也可包含这组值。

在「设置 → 输出与命名 → Emby 媒体写回」添加路径映射。Emby 端目录填写服务器看到的路径，例如 `/media/TV`。桌面版的本地目录填写对应的可写绝对路径，例如 `/Volumes/TV`。多个映射匹配时使用最具体的目录。Docker Web 可用以下方式挂载可写媒体目录，本地目录填写挂载中的相对路径，留空表示挂载根目录。挂载须允许容器用户 UID 65532 写入。

```sh
EMBY_WRITEBACK_MOUNT=/宿主机/媒体目录 docker compose -f compose.yaml -f deploy/compose.emby-writeback.yaml up -d --build
```

剧集根目录写入 `tvshow.nfo` 和可用封面、背景图。电影和分集的 NFO 用真实视频文件名，例如 `第 1 集.mkv` 对应 `第 1 集.nfo`，分集缩略图放在同一目录。Emby 图片文件名遵循[电影](https://emby.media/support/articles/Movie-Naming.html#video-images)和[分集](https://emby.media/support/articles/TV-Naming.html#episode-images)的约定。

1. 采集完成后，在后台任务中点击「预览写回」或「预览批次写回」。核对本机目标路径、新增和替换项。图片采用来源实际返回的格式，预览列出可能的目标文件。
2. 点击「确认写回」。每个现有文件在被替换前都会保存一份备份。目标在预览后发生变化时，请重新预览。整套媒体写回和 Emby 原生「覆盖刮削」是两个独立的操作。
3. 处理路径映射、挂载或权限问题后，点击「查看失败并重试」。此操作使用同一份已保存的元数据，仅处理未成功文件；图片按已确认的写回计划获取。即使媒体不可写，已采集的元数据也可查看和导出。
4. 点击「查看写回记录」可找到被替换文件和同目录 `.collect-metadata-backups/` 中的备份。如需恢复，先暂停该库的写回或自动刷新，将对应备份复制回记录显示的原路径，再让 Emby 重新读取本地资料。备份由部署者按需保留。

家庭「元数据翻译」的旧定时任务在 NAS 处理作品简介和分集标题，并写回 Emby。采集会读取 Emby 已有资料；如需额外的目标语言，可配置上面的翻译适配器。文件夹刮削在 Emby 未匹配到作品时继续查询常规来源。

批次另存时，勾选「Emby NFO」可生成电影、剧集和可匹配分集的 XML。NFO 可写入排序名、原名、简介及原文、发行日期、分级、评分及投票数、演职员、系列、标签、厂牌、图片与预告链接。能获得哪些字段取决于来源。

批次另存的「附加媒体」可分别勾选预告片、主题视频、主题音乐和花絮，支持直链下载。每类每部作品最多下载 10 个文件，单文件上限 256 MiB。TMDB 返回的 YouTube 或 Vimeo 播放页会写入 `media-links.json`，主要预告链接也会写入 NFO。输出反馈会显示哪些已选类型没有文件或仅有链接。

导入 Emby 时，把 `trailers/`、`backdrops/`（主题视频）、`theme-music/` 和 `extras/` 随作品放在同一目录。参见 [Emby 预告片命名](https://emby.media/support/articles/Trailers.html)和 [主题媒体命名](https://emby.media/support/articles/Theme-Songs-Videos.html)。

## Emby 入库自动刮削插件

[`emby-plugin/`](emby-plugin/) 是独立安装在 Emby Server 里的原生插件。Emby 扫描发现新电影、剧集、季、分集或私人影片后，插件会通过 Emby 的后台队列刷新该条目的元数据和图片。给已有剧集添加新分集时，只对新入库的条目提交刷新。插件不会对已有条目做批量回填，也不会请求「替换所有元数据」或「替换所有图片」；已锁定的条目会被跳过。

从 v0.11.0 起，可以直接从 [GitHub Releases](https://github.com/guangl/collect-metadata/releases/latest) 下载 `CollectMetadata.Emby.dll`，并用该资产旁显示的 SHA-256 摘要校验。插件使用 [Emby Server 4.9.1.90 的 SDK](https://www.nuget.org/packages/MediaBrowser.Server.Core/4.9.1.90) 构建，已在 4.10.2.0 Docker 镜像中验证同步。其他版本请先验证兼容性。开发时可用 .NET 8 SDK 构建 `dotnet build emby-plugin/CollectMetadata.Emby.csproj -c Release`。

1. 将 `CollectMetadata.Emby.dll` 复制到 **Emby 服务端数据目录**的 `plugins/`（官方 Docker 镜像中通常是 `/config/plugins/`）。源码构建的 DLL 位于 `emby-plugin/bin/Release/netstandard2.0/`。确保 Emby 进程能读取 DLL，然后重启 Emby。
2. 在 Emby 控制台的「媒体库」为电影和剧集分别启用所需的元数据下载器、图片下载器和首选语言。插件沿用这些设置，不需要 Collect Metadata Web、PostgreSQL 或本应用里配置的 TMDB 密钥。如果未开启实时监控，文件要等计划扫描或手动扫描才会入库。
3. 在测试库放入一部新电影或一个新分集，等 Emby 扫描和后台刷新完成。在 Emby 日志中可看到 `Collect Metadata - Auto Scrape` 记录的 `Requested metadata refresh for …`（表示提交刷新，不代表匹配成功）。元数据结果取决于文件命名、媒体库设置和下载器的连通性。

### 由 Collect Metadata 刮削并写回

部署插件和 Web 服务后，可以给两个容器挂载同一个可写队列目录，并在两边将 \`EMBY_AUTO_SCRAPE_SPOOL\` 设为各自看到的队列绝对路径。插件收到 Emby 新入库事件即入队。Web 服务采集公开来源、写入缺少的 NFO 和图片，再请求 Emby 读取。应用重启时会继续处理队列里的事件。Emby 保持启用本地 NFO 读取。

Web 服务还需要可写的 \`EMBY_WRITEBACK_ROOT\` 和「Emby 媒体写回」路径映射。在 \`EMBY_AUTO_SCRAPE_ROOTS\` 中以分号列出 Emby 看到的普通媒体库根目录，私人影片库使用 \`EMBY_AUTO_SCRAPE_PRIVATE_ROOTS\`。同一 NAS 可以通过 \`EMBY_AUTO_SCRAPE_URL\` 为自动任务指定容器网络内的 Emby 地址，沿用已保存的 API 密钥。保留现有媒体文件和旁挂资料。

文件或剧集目录附上 \`[tmdbid=123]\` 或 \`[bangumiid=123]\` 可以指定作品。遇到无法确认的匹配，查看 Web 服务日志。正式停用其他元数据和图片下载器之前，先在测试库验证新电影和新分集能自动生成资料并被 Emby 读取。

将 Web 的 `EMBY_AUTO_SCRAPE_CENTRAL_METADATA` 设为 `true`，普通影视库入库时会按已确认的来源补齐 TMDB、Bangumi ID 和缺少的图片。该模式需要可用的 PostgreSQL。为对应的 Emby 媒体库关闭「将图片保存到媒体文件夹」和 NFO 元数据保存器，保留 NFO 读取器。Emby 将新图片存入服务器元数据目录，Collect Metadata 在视频目录创建缺少的 NFO、海报、Fanart、Clearlogo、Landscape 或适用的季集图，并按 Emby 服务器和条目 ID 更新 PostgreSQL。缺少可信来源的资料保持为空。更新或重试时保留已存在的旁挂文件。


Emby 的分季与公开目录不一致时，可给 Web 服务挂载一份本地 JSON，并用 `EMBY_AUTO_SCRAPE_SEASON_MAP` 指定容器内文件路径。文件按 Emby **剧集条目 ID → 季号 → 已核对的 Bangumi 条目 ID 列表**编写，例如 `{"123":{"2":[456,789]}}`。同一季上下篇按播出顺序列出。映射使用 Bangumi 的连续分集序号校验集次，并拒绝与已有 Bangumi 分集 ID 或首播日期冲突的结果。季海报取首篇，简介保留已配置篇章的简介。[季集映射规范](docs/EMBY_SEASON_MAPPING.md)

入库事件由插件交给 Collect Metadata。队列写入失败会记录在 Emby 日志。Emby 尚未返回条目的事件等待 15 分钟后归档，其他持续失败的事件等待最多 7 天。归档文件留在队列目录，可根据 Web 日志复核，必要时恢复为原来的 `.event` 文件名。

使用 PostgreSQL 季集映射时，将 Web 的 `EMBY_AUTO_SCRAPE_TIMEZONE` 设为 Emby 本地首播日期所属的 IANA 时区，例如 `Asia/Shanghai`。日期冲突可按[官方放送日复核流程](docs/EMBY_SEASON_MAPPING.md#如何比较首播日期)处理。

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

后台服务会把批次、逐条阶段、候选和结果保存到 PostgreSQL。提交后可点击「查看批次」打开对应任务，也可稍后从主导航进入「后台任务」并选择原批次。完成的资料与任务状态在同一事务保存。

- 「暂停批次」停止处理未完成条目。「继续批次」恢复排队，并恢复原有待选候选。
- 「取消批次」永久停止该批次，已完成的结果仍可查看和导出。
- 「重试全部失败条目」重新排队所有标记为「失败」的条目。批次处于暂停状态时，重试条目会等待手动继续。

服务重启后，暂停和取消状态继续生效。后台只接手运行中批次的任务，原先正在处理的条目会在执行租约到期后重新领取，租约最长 15 秒。

同一个 PostgreSQL 数据库最多同时处理 3 条。连接数据库的桌面端关闭窗口时会保留托盘进程，可从托盘重新打开窗口。退出整个桌面进程后，独立运行的 Web 服务仍可处理同一数据库里的任务。

### 纠正匹配

电影和剧集的后台条目可用「纠正匹配」搜索其他片名或直接填 `tmdb:496243`。Emby 单集使用所属剧集的 TMDB ID 和条目季集号。确认后在原条目重新采集，仍关联相同媒体文件。来源返回有效资料后保存选择，后续同一 Emby 服务的同一条目、同一文件夹分组或同一手动查询会沿用。文件夹批次原有的导出目录会保留，新结果写到带 `-r编号` 的目录。Emby 摘要和图片写回使用现有路径映射，需先预览并确认。

## AI 刮削

给 Web 或桌面进程设置以下环境变量。Docker 从 `.env` 读取，更新后需重建应用容器。模型地址必须能从应用容器访问，例如同一 Docker 网络中的 `http://ollama:11434/v1/chat/completions`。桌面从启动应用的进程读取这些变量。

| 变量 | 值 |
| --- | --- |
| `COLLECT_METADATA_AI_URL` | 完整的 OpenAI 兼容 Chat Completions 地址，如 `http://127.0.0.1:11434/v1/chat/completions` |
| `COLLECT_METADATA_AI_MODEL` | 支持 JSON 对象响应的模型名 |
| `COLLECT_METADATA_AI_API_KEY` | 接口要求的 Bearer 令牌，本地无鉴权服务可留空 |

也可将 `ai.json` 放在与 `tmdb-token` 相同的配置目录。环境变量优先。使用 DeepSeek 时，文件可采用以下结构，并将文件访问权限限制为当前用户。

```json
{
  "url": "https://api.deepseek.com/chat/completions",
  "model": "deepseek-flash",
  "api_key": "填入自己的密钥"
}
```

在媒体库勾选电影或剧集后点「刮削所选」，在弹窗中选择「Collect Metadata AI 刮削」，也可在「手动输入作品」中点「提交 AI 刮削」。已有明确 ID 或唯一精确匹配时直接采集。需要消歧时，模型接收片名、年份及部分公开来源候选，必要时可建议一个新片名重新搜索。可信且年份一致的匹配会采集；需要复核时进入「待选择」。简介、海报和演职员仍来自采集记录中的公开来源或 Emby。模型请求只携带作品检索信息。Emby 的 Video 类型继续读取本地资料。被 Emby 归类为电影的私人库也可以在 `ai.json` 的 `emby_excluded_roots` 数组中填写媒体根目录，保持使用 Emby 资料。

## MCP

通过 [stdio](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#stdio) 将 Collect Metadata Web 接入 MCP 客户端。先运行 Web 服务和 PostgreSQL，再在支持 stdio 的客户端中配置命令。示例适用于接受 `mcpServers` 格式的客户端。

```json
{
  "mcpServers": {
    "collect-metadata": {
      "command": "uv",
      "args": ["run", "--script", "/绝对路径/collect-metadata/tools/collect_metadata_mcp.py"],
      "env": { "COLLECT_METADATA_WEB_URL": "http://127.0.0.1:8080" }
    }
  }
}
```

MCP 可以查询公开候选和 Emby 媒体库、筛缺资料、提交普通或 AI 批次、查进度和读结果，以及纠正匹配。批次与工作台共用。Emby 提交和纠正使用相同写回规则。如果通过配置了 Bearer 鉴权的 HTTPS 反向代理访问远程 Web，可为 MCP 进程提供 `COLLECT_METADATA_WEB_TOKEN`。

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

GitHub CI 在推送 `main` 或提交 PR 时运行前端测试与构建、Rust 桌面和 Web 测试、临时 PostgreSQL 的队列和模拟 Emby 集成测试，以及 Emby 插件构建和整理规则检查。这些检查使用测试配置，无需私有凭据。`npm run test:coverage` 检查 TypeScript 源文件的行覆盖率，门槛为 85%。

Rust 的完整覆盖率检查还包含 PostgreSQL 集成测试及实际来源采集。运行前设置 `COLLECT_METADATA_TEST_POSTGRES`（连接账户需有建 schema 权限）和 `TMDB_READ_ACCESS_TOKEN`，并确保能连接测试使用的公开来源。

```sh
npm run build
cargo llvm-cov clean --workspace
EMBY_SYNC_TOKEN=fixture-key cargo llvm-cov --locked --all-features --summary-only --fail-under-lines 85 -- --include-ignored --test-threads=1
```

## Docker Web 版

从 [`.env.example`](.env.example) 复制本地 `.env` 并设置 `POSTGRES_PASSWORD`。以下两种部署方式都会启动 Web 应用和 PostgreSQL，并把数据库数据保存在 `postgres_data` 卷中。

可选的 Compose 配置位于 `deploy/`。更新已有部署时，在每个可选配置文件名前加 `deploy/`，例如将 `-f compose.ghcr.yaml` 改为 `-f deploy/compose.ghcr.yaml`，并沿用原 Compose 项目名。

**使用预构建镜像（Compose v2.24.4+）：** 从 v0.11.0 起，发布流程为 amd64 和 arm64 构建镜像。`ghcr.io/guangl/collect-metadata:latest` 随最新 GitHub Release 更新。需要固定部署版本时，在 `.env` 设置 `COLLECT_METADATA_IMAGE=ghcr.io/guangl/collect-metadata:0.12.2`，也可用镜像摘要固定到具体构建。[GitHub Container Registry](https://github.com/users/guangl/packages/container/package/collect-metadata)

```sh
docker compose -f compose.yaml -f deploy/compose.ghcr.yaml pull
docker compose -f compose.yaml -f deploy/compose.ghcr.yaml up -d
```

如果拉取时返回 `denied`，请检查镜像标签是否已发布，以及 GHCR 包的可见性；私有包需要先用有读取权限的账号登录 `ghcr.io`。也可以改用下面的方式从源码构建。

**从源码构建：**

```sh
docker compose up -d --build
```

浏览器打开 [http://127.0.0.1:8080](http://127.0.0.1:8080)。在 `.env` 中用 `APP_HOST` 和 `APP_PORT` 调整宿主机监听地址和端口。

直接运行 Web 服务器时，如需后台任务或采集历史，可通过 `DATABASE_URL` 或标准 `PGHOST`、`PGPORT`、`PGDATABASE`、`PGUSER`、`PGPASSWORD` 连接已有 PostgreSQL。配置数据库后，应用启动时会创建 `collect_metadata_snapshots` 表。每次有结果的采集写入一条 JSONB 快照，重新采集会新增记录。不要将应用或数据库端口直接暴露在公网，详见 [安全说明](.github/SECURITY.md)。

```sh
npm ci
npm run build
cargo run --locked --no-default-features --features web --bin collect-metadata-web
```

## 升级

### 从 v0.9.0 升级

1. 暂停新的采集和写回，等待正在执行的任务收尾。备份 PostgreSQL 和应用配置；Emby 插件如需更新，保留安装目录里原来的 DLL。确认桌面端和 Docker Web 端是否共用数据库，一起升级这些进程。
2. 桌面版用新版本的 DMG 或 EXE 安装。Docker Web 在原目录用相同的 Compose 文件、项目名和挂载启动。镜像部署先更新 `COLLECT_METADATA_IMAGE`，再 `pull` 和 `up -d --wait`；源码部署使用 `up -d --build --wait`。应用启动时自动添加写回字段，沿用已有的历史、队列和设置。
3. 新的 Emby 刮削批次要在后台任务中预览并确认写回。既有批次保留原来的输出报告；旧版没有逐文件写回记录，若需要按新流程重试，重新提交所需作品。Emby 插件可独立升级，替换 DLL 后重启 Emby。
4. 验证 `/readyz`、历史列表和一个测试作品的写回预览。需要回退时，固定原镜像或重装原安装包，并把升级前的备份恢复到独立的数据库和配置副本后再切换。

### NAS 健康检查

桌面版和 Web 版可在「设置 → 日志与排查」查看日志位置和下载排查手册。用诊断编号或批次编号查找相关事件。详见[报错排查手册](docs/TROUBLESHOOTING.md)。

- `GET /healthz` 返回 204，代表 Web 进程可以响应。
- `GET /readyz` 在限时检查已配置的 PostgreSQL 后返回 204 或 503。独立 Web 未配置数据库时返回 204。Docker 镜像用它报告容器健康状态；NAS 上的外部监控也可请求这个地址。
- `docker compose ps` 可确认容器状态。若就绪检查失败，查看 `docker compose logs --tail=100 app db`，并确认数据库容器和连接配置。媒体路径和写回权限可从具体批次的预览与失败记录检查。

### 备份与恢复

下面以 Docker Web 为例。有 Compose 覆盖文件的部署继续使用原有的 `-f` 参数。数据库卷保存快照、队列和同步数据；`tmdb_config` 卷也包含 Emby、语言和输出设置，以及队列使用的本机标识。备份中可能含数据库和服务凭据，请放在受限目录。

在例行维护时先关闭使用同一库的桌面进程。下面的命令停止 Web 应用，把数据库、配置卷和 Compose 环境文件备份到一个新目录，完成后重新启动应用。

```sh
umask 077
backup_dir="$PWD/collect-metadata-backup-$(date +%Y%m%d-%H%M%S)"
(
  set -e
  mkdir "$backup_dir"
  docker compose stop app
  trap 'docker compose up -d --wait' EXIT
  docker compose exec -T db sh -c 'exec pg_dump -Fc -U "$POSTGRES_USER" "$POSTGRES_DB"' \
    > "$backup_dir/database.dump"
  docker compose run -T --rm --no-deps --user 0 --entrypoint tar app \
    -C /var/lib/collect-metadata -czf - . > "$backup_dir/app-config.tar.gz"
  cp .env "$backup_dir/compose.env"
  touch "$backup_dir/COMPLETE"
)
```

使用带有 `COMPLETE` 标记的备份目录。另行保留使用中的 Compose 覆盖文件。需要保留导出文件时，备份 `scrape_output` 或 `SCRAPE_OUTPUT_MOUNT`。媒体目录及其中的写回备份随 NAS 媒体备份保存。独立 PostgreSQL 可用同样的 `pg_dump -Fc` 备份，桌面端还要复制应用配置目录。

恢复时，用一个新的 Compose 项目和未使用的端口验证。下面的命令只使用基础 Compose 文件，避免恢复演练写回原媒体目录。由于恢复出来的队列会在服务启动后继续处理，演练前请在原部署中暂停待处理批次再备份，或在测试网络隔离实际 Emby 和翻译服务。在仓库根目录执行，沿用上面的 `backup_dir`。

```sh
(
  set -e
  test -f "$backup_dir/COMPLETE"
  export APP_PORT=18080
  docker compose --env-file "$backup_dir/compose.env" -p collect-metadata-restore up -d --wait db
  docker compose --env-file "$backup_dir/compose.env" -p collect-metadata-restore exec -T db \
    sh -c 'exec pg_restore --exit-on-error --single-transaction --no-owner -U "$POSTGRES_USER" -d "$POSTGRES_DB"' \
    < "$backup_dir/database.dump"
  docker compose --env-file "$backup_dir/compose.env" -p collect-metadata-restore run -T --build --rm \
    --no-deps --user 0 --entrypoint tar app -C /var/lib/collect-metadata -xzf - \
    < "$backup_dir/app-config.tar.gz"
  docker compose --env-file "$backup_dir/compose.env" -p collect-metadata-restore up -d --wait
  curl -fsS http://127.0.0.1:18080/readyz
)
```

核对历史、批次、设置和输出文件，再切换实际部署。恢复到原服务前，请先备份当前服务的数据库和配置。

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

桌面版无需连接数据库即可刮削本地文件夹。需要将任务留在后台运行、继续已有批次或查看采集历史时，可以连接 PostgreSQL。在「设置」的「PostgreSQL」中粘贴连接地址，点击「测试连接并保存」。也可在启动前设置 `DATABASE_URL`，或在下列路径写入一行连接串。环境变量优先。Web 和桌面版都可以从「设置」测试当前数据库，Web 的连接地址由服务端环境变量提供。

如需桌面版和 Docker Web 版共用本机数据库，可以加上桌面用 Compose 配置。它仅在本机回环地址开放 PostgreSQL 端口。

```sh
docker compose -f compose.yaml -f deploy/compose.desktop.yaml up -d --build
```

桌面端连接 `127.0.0.1:15432` 的 `collect_metadata` 数据库，用户名为 `collect_metadata`，密码取 `.env` 中的 `POSTGRES_PASSWORD`。可用 `POSTGRES_LOCAL_PORT` 调整端口。

| 系统 | `database-url` 路径 |
| --- | --- |
| macOS | `~/Library/Application Support/com.collectmetadata.desktop/database-url` |
| Windows | `%APPDATA%\com.collectmetadata.desktop\database-url` |

## 许可证

本项目代码采用 [Apache License 2.0](LICENSE)。第三方元数据、图片和商标不因本项目的许可证而获得授权，使用时应遵守各来源的条款。
