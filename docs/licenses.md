# 许可与第三方来源

hproxy 主控、agent、前端代码及项目脚本按仓库根目录的 [GPL-3.0-or-later](../LICENSE) 发布。源文件中未单独标注时，以工作区 `Cargo.toml` 的许可证声明为准。贡献者提交代码应确认有权按该许可证发布。

仓库和发布包同时包含第三方材料，使用时应保留对应版权和许可声明：

| 材料 | 来源与说明 | 许可位置 |
| --- | --- | --- |
| Realm 补丁源码 | 本仓库 `vendor/realm`，上游作者 zhboner；项目修改动态 DNS、流量统计等代码 | [`vendor/realm/LICENSE`](../vendor/realm/LICENSE)，MIT |
| sing-box 静态发布二进制 | 构建脚本固定从 [SagerNet/sing-box v1.14.3](https://github.com/SagerNet/sing-box/releases/tag/v1.14.3) 下载并校验 SHA-256；不在源码库中提交其二进制 | [上游仓库的许可与声明](https://github.com/SagerNet/sing-box)；使用发布包时请同时阅读上游条款 |
| 国旗后备图形 | Twemoji 17.0.3 的原始 SVG，仅在系统不支持彩色旗帜时使用 | [`SOURCES.md`](../web/public/flag-emoji/SOURCES.md) 和 [`LICENSE-GRAPHICS`](../web/public/flag-emoji/LICENSE-GRAPHICS)，CC BY 4.0 |
| Rust/npm 依赖 | `Cargo.lock` 与 `web/package-lock.json` 固定实际构建依赖 | 各依赖的上游许可文件 |

自动发布的 OCI 镜像包含项目主控或 agent 二进制、Alpine 基础层及 CA 证书，并可能包含 sing-box/Realm 发布包。分发镜像或发布包时仍须遵守这些组件各自的许可条件；本页不是对第三方许可的重新授权。
