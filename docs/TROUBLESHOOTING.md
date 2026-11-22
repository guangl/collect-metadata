# Collect Metadata 报错排查手册

## 先找到对应日志

1. 记下应用版本、报错原文、发生时间和时区。提示有「诊断编号」时复制完整编号。后台任务请记录「批次 #」和「条目 #」。
2. 在「设置 → 日志与排查」找到日志。查看报错前后几分钟的事件。Web 接口的编号也在响应头 `X-Request-ID` 中。JSON 日志的 `timestamp` 使用 UTC。
3. 修正后先用一部作品或一个测试文件重试。已有结果时先打开作品详情、输出反馈或写回预览，确认失败发生在哪一步。

| 运行方式 | 日志位置 |
| --- | --- |
| macOS 桌面版 | 从设置打开日志目录，或在访达中前往 `~/Library/Logs/com.collectmetadata.desktop/` |
| Windows 桌面版 | 从设置打开日志目录，或在资源管理器中打开 `%LOCALAPPDATA%\com.collectmetadata.desktop\logs` |
| Linux 桌面版 | 以设置显示的目录为准，默认位于本地数据目录下的 `com.collectmetadata.desktop/logs/` |
| Docker Web | 在部署主机执行 `docker compose logs --since 30m`，再按需指定服务名 |
| 直接运行 Web | 查看启动终端或进程管理器收集的标准错误 |
| Emby 插件 | 在 Emby 管理后台「日志」搜索 `Collect Metadata`，并查看对应计划任务的结果 |
| 翻译桥接、MCP、私人影片 worker | 查看各自的启动终端、进程管理器或容器日志 |

桌面保留 `collect-metadata.log` 和 `collect-metadata.1.log`、`collect-metadata.2.log`，每份约 2 MiB。写入失败时应用会尝试输出到标准错误。Compose 默认给 `app` 保留三个约 10 MiB 的日志文件，以部署配置为准。Compose 命令请沿用原部署的 `-f` 和项目名参数。可用 `docker compose ps --services` 确认服务名。下例使用默认服务 `app` 和 `db`，自定义部署请替换为实际名称。

```sh
docker compose ps
docker compose logs --since 30m app db
docker compose logs --since 30m app | rg -F '完整诊断编号'
docker compose logs --since 30m app | rg '"batch_id":42[,}]' | rg '"row_id":17[,}]'
```

`42` 和 `17` 是示例编号。也可在文本编辑器中搜索。没有编号时，查看浏览器网络面板中的状态码和响应头。浏览器本地失败、网络中断或代理拦截可能发生在 Web 应用收到请求以前。

## 启动、网络与 PostgreSQL

| 表现或日志事件 | 排查顺序 |
| --- | --- |
| 桌面启动即退出；`desktop_setup_failed` | 查看最近一次启动的日志和配置阶段。检查当前用户能读取应用配置目录。近期手工改过 AI、语言、Emby 或输出配置时，先备份对应文件，再核对 JSON 格式。 |
| Web 无法连接；`web_bind_failed` | 看 `docker compose ps` 和启动日志。直接运行时检查 `APP_BIND`；Compose 检查 `APP_HOST`、`APP_PORT` 和宿主机端口占用。代理部署先从部署主机直接访问应用端口，再查代理的上游地址。 |
| `/healthz` 204，`/readyz` 503 | Web 已能响应，数据库检查失败或超时，查 `web_readiness_failed`。先看 `app` 和 `db` 日志，再确认数据库允许从执行实例连接。应用启动时初始化数据库失败，两个端点都可能无法访问。 |
| `postgres_connect_failed`、`postgres_check_failed`、`batch_store_unavailable` | 桌面在「设置 → PostgreSQL」重新测试；Web 检查生效的 `DATABASE_URL` 或 `PG*`。Compose 内部默认使用 `db:5432`。按 `reason` 和 `sqlstate` 检查网络、TLS、认证或连接池。 |
| `postgres_migration_failed` | 确认配置指向预期数据库和 schema。核对账号是否有创建应用表的权限，是否能修改已有应用表。 |
| 采集完成但保存失败；`postgres_snapshot_save_failed` | 先导出当前结果，恢复数据库连接后再采集，以保存历史。 |
| 历史、队列或结果读取失败 | 按编号查 `web_request_failed` 或 `desktop_command_failed`，再看同编号是否有 PostgreSQL 事件。核对数据库和对应记录是否存在。 |
| HTTP 401、403、404、405、413、429 或 5xx | 401/403 查登录和权限，插件同步查令牌。404/405 核对代理路径和页面、API 版本。413 缩小提交范围并核对代理限制。429 稍后重试。5xx 查代理上游、应用与数据库。 |

```sh
curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/healthz
curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/readyz
docker compose exec db pg_isready -U collect_metadata -d collect_metadata
```

将 8080 换成实际发布端口。`pg_isready` 检查数据库是否在接受连接，认证和权限以应用测试为准。常见 SQLSTATE 有 `28P01`（密码认证失败）、`3D000`（数据库不存在）、`42501`（权限不足）、`53300`（连接数过多）。修改 Web 环境变量后用原部署参数重建应用容器。已有 PostgreSQL 数据卷的用户密码由数据库管理，在 `.env` 改动 `POSTGRES_PASSWORD` 后仍需核对数据库实际密码。

## 队列、采集来源、AI 与翻译

| 表现或日志事件 | 排查顺序 |
| --- | --- |
| 一直排队 | 检查批次是否暂停，是否有任务占用三个共享执行槽。桌面和 Web 共用数据库时，带另存、Emby 导入或写回执行条件的任务需要由对应实例领取。要求 TMDB 或 AI 的任务也需要满足条件的实例在运行。 |
| 处理中反复重来；`batch_claim_failed`、`batch_lease_renew_failed`、`batch_commit_failed` | 查看该条目 `attempt` 是否持续增加，同时检查数据库日志、连接数和磁盘。恢复后查看 `batch_queue_recovered` 或对应的 `batch_row_finished`。 |
| 「待选择」、匹配错误或「未找到」 | 核对类型、原片名、年份、集号或番号；用「纠正匹配」或试用明确 ID，如 `tmdb:496243`、`bangumi:253`。 |
| 「部分完成」 | 展开作品详情看来源提示，再看条目的输出反馈。已取得的资料可以使用，缺失可能来自个别来源、分集、图片或输出文件。 |
| 外部来源失败；`source_request_failed`、`source_detail_unavailable` | 查 `provider`、HTTP 状态和 `reason`。从运行应用的主机或容器排查网络、DNS 和代理。401/403 核对凭据或站点限制，429 减少频率，5xx 等来源恢复。能访问站点但解析失败时检查来源是否改版或要求登录。 |
| TMDB 查不到或连接失败 | 到「设置 → TMDB」测试当前凭据，确认是 API Read Access Token 或 v3 API Key。共享数据库时还要检查领取任务的进程。 |
| AI 按钮不可用、超时或结果无效 | 查执行实例的 `COLLECT_METADATA_AI_URL`、`COLLECT_METADATA_AI_MODEL`、所需 API Key 或本机 `ai.json`；URL 要指向完整的 chat completions 接口。查看 `ai_request_failed` 和 `ai_match_failed`，可先手动选择来源验证普通采集。 |
| 已配翻译但仍显示原文 | 核对目标语言、翻译桥接 URL 和令牌。查 `translation_request_failed`。401 查令牌，429 查每日额度，503 查是否繁忙，502 查 Sublarr/DeepSeek。桥接返回空译文时沿用原文。 |

## 文件夹、导出与写回

| 表现 | 排查顺序 |
| --- | --- |
| 服务器目录不可见、扫描失败 | 桌面确认系统已挂载且当前用户可访问。Web 直接运行时查 `SCRAPE_MEDIA_ROOT`；Docker 查 `deploy/compose.media.yaml` 和 `SCRAPE_MEDIA_MOUNT`，容器 UID 65532 要能读取目录并进入各级路径。浏览器选择目录时以浏览器所在机器能看到的文件为准。 |
| 零作品、文件被跳过或提交太大 | 核对视频扩展名、片名年份和季集号。样片、花絮和非视频会跳过。每次最多扫描 30,000 个文件、提交 2,000 部文件夹作品；手动和 Emby 勾选上限以界面为准。 |
| 设置保存、JSON 导出或另存失败 | 检查配置或目标目录权限、可用空间、挂载是否断开。另存请核对提交批次时的设置、条目输出反馈和 `files.json`。只有预告链接时，确认来源是否提供直链。 |
| 无法预览写回或路径不符 | 对照 Emby 中的媒体路径和「输出与命名 → Emby 媒体写回」映射。桌面检查本机绝对路径；Docker 检查 `deploy/compose.emby-writeback.yaml`、映射的相对路径及 UID 65532 权限。 |
| 写回失败或预览已失效；`batch_writeback_finished` 有失败项 | 点「查看失败并重试」看每项原因。核对只读挂载、权限、原视频是否仍在、目标是否被改动。修好后重新预览和确认。 |
| `batch_writeback_snapshot_missing` 或 `batch_writeback_snapshot_read_failed` | 检查条目详情能否打开、数据库健康状态及是否连接到原数据库。先保留现有记录，确认快照状态后再决定是否重新采集。 |
| 恢复被替换的文件 | 暂停相关写回和 Emby 自动刷新，从「查看写回记录」定位原件及同目录 `.collect-metadata-backups/` 下的备份。核对后恢复对应原件，再让 Emby 重新读取本地资料。 |

## Emby 插件、同步与 MCP

| 表现 | 排查顺序 |
| --- | --- |
| 读取 Emby 失败；`emby_request_failed` | 在「设置 → 家庭 Emby」测试地址和密钥。检查执行实例能否访问 Emby，核对容器网络、代理子路径、协议和权限。 |
| Emby 图片或部分分集缺失 | 比对同一条目在 Emby 中是否有海报、图片或分集。出现「部分分集暂未读取」时，检查 Emby 连通性和服务端日志。 |
| 「覆盖刮削」提交后内容没变 | 到 Emby 查看刷新任务和日志，确认媒体库启用了所需下载器且能连接来源。应用的提示表示刷新请求已提交。 |
| 入库后没有自动刮削 | 看 `Collect Metadata - Auto Scrape` 和 Emby 库扫描日志，确认 DLL 已加载、实时监控或计划扫描已运行、条目可刷新。 |
| 散文件未自动整理 | 在「计划任务」检查 `Organize loose library videos`。核对实际库根、NFO 保存器、将图片保存到媒体文件夹、写入权限和文件稳定等待时间。 |
| 同步到 PostgreSQL 失败 | 在 Emby 运行 `Sync library to Collect Metadata`。检查 Web 的 `/readyz`、两个容器之间的地址、双方 `EMBY_SYNC_TOKEN`。401 查令牌，400/413 查被拒绝的条目或请求大小，503 查同步配置和数据库。用 Emby 日志里的诊断编号关联 Web，查看 `emby_sync_rejected` 或 PostgreSQL 事件。 |
| MCP 调用失败 | 核对 MCP 进程的 `COLLECT_METADATA_WEB_URL`、所需访问令牌及网络。用 MCP 返回的 HTTP 状态和诊断编号关联 Web。 |
| 私人影片 worker 失败 | 查看独立 worker 的启动输出和任务摘要，核对番号识别、来源匹配、NFO 和剧照目录写权限。先用测试目录复现。 |

反馈时附上版本、时间和时区、运行方式、触发步骤、诊断编号或批次与条目编号，以及附近几行日志。分享 Emby、代理或独立 worker 的日志、请求截图和导出内容前，删去凭据、媒体路径、私人作品名和完整请求内容。
