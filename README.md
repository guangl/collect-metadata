# Collect Metadata

[![CI](https://github.com/guangl/collect-metadata/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/guangl/collect-metadata/actions/workflows/ci.yml)

**给本地文件夹和 Emby 媒体库用的自托管元数据刮削工具。** 在一个工作台里预览作品、提交后台任务、查看结果并导出资料，不移动或改写原视频。

[快速上手](#快速上手) · [桌面版](#tauri-桌面版) · [Emby 插件](#emby-入库自动刮削插件) · [贡献指南](CONTRIBUTING.md) · [安全报告](SECURITY.md)

- **导入**：选择本地文件夹、服务端挂载目录或 Emby 媒体库，也可手动输入作品清单。
- **后台采集**：支持暂停、继续和重试；有多个匹配时可手动确认。任务与历史保存在 PostgreSQL。
- **查看与导出**：按作品类型或原始语言筛选历史，查看海报和来源字段，导出 JSON、图片或 Emby NFO。

## 快速上手

有 Docker 和 Compose 时，先克隆仓库并复制配置示例：

```sh
git clone https://github.com/guangl/collect-metadata.git
cd collect-metadata
cp .env.example .env
```

在 `.env` 中为 `POSTGRES_PASSWORD` 设置新密码，然后运行 `docker compose up -d --build`。打开 [http://127.0.0.1:8080](http://127.0.0.1:8080)，选择文件夹，确认预览后点「开始刮削元数据」。部署与存储选项见 [Docker Web 版](#docker-web-版)。

macOS 和 Windows 也可从源码运行 [Tauri 桌面版](#tauri-桌面版)。Web 与桌面共用 React + TypeScript 界面和 Rust 采集核心。

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

打开应用后，在「媒体来源」选择「文件夹」或「Emby Server」。文件夹模式递归识别子目录和视频文件名，按作品分组显示「媒体海报预览」。预览不创建批次；检查后点「开始刮削元数据」才提交后台任务，队列继续用海报卡片显示进度。

| 文件路径 | 识别结果 |
| --- | --- |
| `电影/寄生虫 (2019)/寄生虫.2019.2160p.mkv` | 电影《寄生虫》，年份 2019 |
| `暗黑 (2017)/Season 01/S01E02.mkv` | 剧集《暗黑》，年份 2017 |
| `AV/SSIS-001.1080p.mp4` | 番号 `SSIS-001` |
| `电影/[tmdb-496243].mkv` | 使用指定的 TMDB ID |

扫描会去掉常见分辨率、编码和发行标签。同一部作品的多集或多个版本归到同一条，样片、花絮和非视频文件会跳过。同名作品按文件名或目录名里的年份匹配。如果结果有歧义，可在该条进度里选择作品。每次最多扫描 30,000 个文件、提交 2,000 部作品。

桌面版使用本地文件夹选择器，Web 只读取所选目录中的文件名和相对路径。预览无需 PostgreSQL；提交刮削后，系统才把识别结果、进度和关联文件路径存入 PostgreSQL。整理与刮削都不会移动、重命名或修改原视频。

### NAS / SMB / NFS 共享目录

应用读取**已经挂载的**网络共享，不直接登录 SMB 或 NFS 服务，也不保存网络磁盘账号和密码。

- **桌面版**：先在系统挂载共享，再用「选择媒体文件夹」选取。macOS 的 SMB 共享可在访达用「连接服务器」挂载；Windows 可在资源管理器映射网络驱动器。NFS 使用操作系统的 NFS 客户端挂载。选盘体验取决于系统是否能访问挂载目录。
- **Web 版**：可用浏览器选择客户端能访问的目录，也可在「文件夹 → 浏览服务器挂载目录」中选择服务端的共享。直接运行 Web 服务时，给 `SCRAPE_MEDIA_ROOT` 设置一个已挂载的绝对路径。Docker 部署时，先在宿主机挂载共享，在 `.env` 设置 `SCRAPE_MEDIA_MOUNT=/宿主机/共享目录`，再运行：

  ```sh
  docker compose -f compose.yaml -f compose.media.yaml up -d --build
  ```

  Docker 将该目录只读挂载到容器的 `/media-input`。容器用户 UID 65532 需要读取文件名和访问子目录的权限。只挂载准备刮削的目录，不要暴露 NAS 根目录；能访问此 Web 界面的用户能浏览该共享范围内的目录。服务端目录选择器不接受共享根目录之外的路径。

### 输出文件

在「设置 → 输出与命名」打开「写出刮削文件」，选择输出路径、作品目录名、资料文件名前缀和资料类型。可以分别输出作品 JSON、原始来源 JSON、海报、背景图、剧照及分集截图。作品 JSON 可勾选简介、演职员、评分和季集。人物头像可按演员、声优、导演、编剧及其他幕后人员选择，是否有头像取决于来源。

设置随新的文件夹批次一起保存。后台在所选目录下为每批建立 `batch-编号/work-编号`。作品目录名和资料文件名前缀可使用 `{title}`（片名）、`{year}`（年份）、`{id}`（来源 ID）、`{type}`（作品类型）、`{row}`（条目编号）。自定义作品目录名会附加条目编号。资料文件名前缀应用于 JSON 和图片。`files.json` 记录文件、数据来源及对应的媒体文件，Emby NFO 沿用媒体文件名。每部作品最多写出 20 张剧照和 100 项人物头像，队列里会显示输出路径和写入反馈。已排队的输出任务由提交它的桌面应用或 Web 服务继续执行。

桌面版使用目录选择器。Docker Web 使用挂载到容器 `/output` 的 `scrape_output` 卷，可在页面填写其中的相对子目录。若要直接写入宿主机文件夹，在 `.env` 设置 `SCRAPE_OUTPUT_MOUNT=/宿主机/输出路径` 并重建应用容器。该目录须允许容器内的 UID 65532 写入。

### Emby 和家庭元数据翻译

在「设置 → 家庭 Emby」填写服务地址和 API 密钥，测试后启用「优先读取 Emby 媒体库」。文件夹刮削会按片名、年份或明确的 TMDB ID 查找已有作品。读取 Emby 返回的完整作品资料与季集记录，依照输出设置写出海报、剧照和人物头像。启用原始来源 JSON 后，输出中会包含 Emby 返回的源记录。

也可以在刮削首页选择「Emby Server」，读取媒体库列表，选中一个库并点「预览所选媒体库」。预览读取片名用于占位海报；确认开始后才提交后台任务，并逐项读取电影、剧集和季集资料，适用相同的输出选项。即使关闭「优先读取 Emby 媒体库」，只要已配置连接，仍可手动选择 Emby Server。

家庭「元数据翻译」的旧定时任务在 NAS 处理作品简介和分集标题，并写回 Emby。采集会读取 Emby 已有资料；如需额外的目标语言，可配置上面的翻译适配器。Emby 是只读资料源，未匹配到作品时继续查询常规来源。

勾选「Emby NFO」可另存 XML 格式的电影、剧集与可匹配分集的 NFO。使用时把电影或分集 NFO 放到对应视频旁边，名称与视频保持一致。剧集的 `tvshow.nfo` 放到剧集根目录。输出默认集中到指定目录，便于检查后再导入媒体库。

## Emby 入库自动刮削插件

[`emby-plugin/`](emby-plugin/) 是独立安装在 Emby Server 里的原生插件。Emby 扫描发现新电影、剧集、季或分集后，插件会通过 Emby 的后台队列刷新该条目的元数据和图片。给已有剧集添加新分集时，只对新入库的条目提交刷新。插件不会对已有条目做批量回填，也不会请求「替换所有元数据」或「替换所有图片」；已锁定的条目会被跳过。

使用 [Emby Server 4.9.1.90 的 SDK](https://www.nuget.org/packages/MediaBrowser.Server.Core/4.9.1.90) 构建，已在 4.10.2.0 Docker 镜像中验证同步。其他版本请先验证兼容性。构建需要 .NET 8 SDK 或更新版本：

```sh
dotnet build emby-plugin/CollectMetadata.Emby.csproj -c Release
```

1. 只把 `emby-plugin/bin/Release/netstandard2.0/CollectMetadata.Emby.dll` 复制到 **Emby 服务端数据目录**的 `plugins/`（官方 Docker 镜像中通常是 `/config/plugins/`）。确保 Emby 进程能读取 DLL，然后重启 Emby；不要复制任何 SDK DLL。
2. 在 Emby 控制台的「媒体库」为电影和剧集分别启用所需的元数据下载器、图片下载器和首选语言。插件沿用这些设置，不需要 Collect Metadata Web、PostgreSQL 或本应用里配置的 TMDB 密钥。如果未开启实时监控，文件要等计划扫描或手动扫描才会入库。
3. 在测试库放入一部新电影或一个新分集，等 Emby 扫描和后台刷新完成。在 Emby 日志中可看到 `Collect Metadata - Auto Scrape` 记录的 `Requested metadata refresh for …`（表示提交刷新，不代表匹配成功）。元数据结果取决于文件命名、媒体库设置和下载器的连通性。

### 自动同步到 PostgreSQL

插件只连接 Collect Metadata Web 服务，**不需要 PostgreSQL 地址、账号或密码**。配置方法：

1. 在 Collect Metadata 的 `.env` 中设置 `EMBY_SYNC_TOKEN`（建议用 `openssl rand -hex 32` 生成），然后 `docker compose up -d --build`。不要提交包含令牌的文件。
2. 在 **Emby 容器**设置 `COLLECT_METADATA_URL`（从 Emby 容器能访问的 Collect Metadata 服务根地址）和相同的 `EMBY_SYNC_TOKEN`，重建 Emby 容器使环境变量生效。两个容器在同一个 Docker 网络时，可用 `http://app:8080`（Compose 默认服务名）；不要用 `localhost` 指向另一个容器。如果经过 NAS 发布端口，需将 `APP_HOST` 绑定到 Emby 能访问的主机接口。跨不可信网络请使用 HTTPS。非 Docker 安装也可在重启前设置同名环境变量，或在 Emby 的 `plugins/configurations/CollectMetadata.Emby.xml` 中配置 `<SyncConfiguration><CollectMetadataUrl>服务地址</CollectMetadataUrl><SyncToken>同步令牌</SyncToken></SyncConfiguration>`。
3. 初次配置后，在 Emby 控制台「计划任务」中运行 **Sync library to Collect Metadata**。之后默认每 30 分钟同步一次；也可在该任务里调整频率。服务不可用时，下一次运行会重新推送，不需要手工补单。

任务同步已有和新增的电影、剧集、季及分集（不含虚拟占位项和花絮）；Emby 内更新的简介、标题等会在下次同步时更新。PostgreSQL 的 `collect_metadata_emby_items` 按 `(server_id, item_id)` 保存最新的元数据 JSON，图片只保存 Emby 的引用，不传图片或视频文件。该表独立于手动采集的历史快照。重复同步不新增行；从 Emby 删除条目不会自动删库。

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
番号：ABC-123
```

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

从 [`.env.example`](.env.example) 复制本地 `.env` 并设置 `POSTGRES_PASSWORD`。以下命令会启动 Web 应用和 PostgreSQL，并把数据库数据保存在 `postgres_data` 卷中。

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

在对应平台打包，可在 `target/release/bundle/` 找到产物。

```sh
# macOS
npm run tauri -- build --bundles app,dmg

# Windows
npm run tauri -- build --bundles nsis
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
