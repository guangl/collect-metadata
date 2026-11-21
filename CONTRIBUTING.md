# 参与贡献 / Contributing

欢迎通过 [Issue](https://github.com/guangl/collect-metadata/issues/new/choose) 报告问题或讨论改进。新资料源、数据库模型或批次行为的改动，建议先提 Issue 对齐范围。

## 本地检查

准备 Node.js 24、Rust 1.100，以及 [Tauri 平台依赖](https://v2.tauri.app/start/prerequisites/)。在仓库根目录运行：

```sh
npm ci
npm run test:coverage
npm run build
cargo test --locked --all-features
```

默认检查不需要私有凭据，也不依赖真实媒体库。需要 PostgreSQL 或在线资料源的测试带有 `ignore`；运行方法见 [README 的测试章节](README.md#测试)。新增测试优先使用脱敏样例或本地模拟服务。

## 提交 Pull Request

1. Fork 仓库，从 `main` 创建分支，每个 PR 尽量只解决一个问题。
2. 为行为变更补测试；配置或使用方式变化时同步更新文档。
3. 在 PR 中说明改动、验证结果，以及是否需要迁移。不要提交 `.env`、令牌、数据库导出或真实媒体路径。

安全问题请走 [私密报告流程](SECURITY.md)，不要发到公开 Issue。
