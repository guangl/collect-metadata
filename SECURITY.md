# 安全报告 / Security Policy

请把疑似漏洞提交到 [GitHub 私密安全报告](https://github.com/guangl/collect-metadata/security/advisories/new)，不要先在公开 Issue、PR 或日志附件中披露。

报告请注明受影响的版本或提交、运行方式、影响范围与最小复现步骤。请脱敏：不要附上真实令牌、PostgreSQL 连接串、NAS 路径或私人媒体信息。非安全问题请使用 [Issue 模板](https://github.com/guangl/collect-metadata/issues/new/choose)。

Docker Web 默认只在本机回环地址监听。应用没有面向公网的用户认证层；如需在可信局域网之外访问，应通过配置了访问控制和 HTTPS 的反向代理，不要直接暴露应用端口或 PostgreSQL。为媒体输入配置的目录应只读挂载，并限制在确实要处理的范围。
