# Coding Tools MCP Android 壳

一个小型原生 Android 应用，用 WebView 打开电脑端的浏览器分享页面。Android 8.0（API 26）及以上。

## 使用

1. 在电脑端开启“浏览器分享”，复制**分享链接和分享口令**。它们与 MCP OAuth 授权密码不同。
2. 安装 APK，输入链接和口令，点“连接工作台”。应用不内置任何服务器或凭据。
3. 勾选“加密记住口令”后，下次启动自动连接。链接与口令使用 Android Keystore AES-256-GCM 加密保存，并排除备份/设备迁移。
4. 网页占满状态栏、刘海、键盘和底部手势区域之外的可用空间，不增加常驻工具栏。返回键先返回网页上一页，在首页返回连接设置。
5. “清除已保存连接”删除保存内容和网页存储。HTTP 需要用户主动允许；建议公网使用 HTTPS。证书错误不会被忽略。

原生登录使用现有 `/browser/login` 接口，发送对应 `Origin`；不跟随登录重定向。网页仅得到同一来源的会话令牌，保存的分享口令不会注入网页，也没有 JavaScript 原生桥。应用支持系统文件选择器上传，不申请整个存储空间权限。

当前版本面向桌面客户端的浏览器分享协议，不把 Node Agent 的本机管理密钥当作分享口令。外部网站在浏览器打开。网页内依赖 Blob URL 的下载暂未提供原生下载适配；需要时可在系统浏览器下载。

## 构建与验证

工具链：JDK 17、Gradle 8.13、Android Gradle Plugin 8.9.3、Android SDK/Build Tools 35。

```sh
export ANDROID_HOME=/path/to/android-sdk
./gradlew :app:testDebugUnitTest :app:lintDebug :app:assembleDebug :app:assembleRelease
```

Windows 使用 `gradlew.bat`。Gradle 分发包的 SHA-256 已固定在 wrapper 配置中。`local.properties`、签名密钥和构建产物不提交。

- 已签名开发包：`app/build/outputs/apk/debug/app-debug.apk`。
- 未签名 release：`app/build/outputs/apk/release/app-release-unsigned.apk`。
- 对 release 使用 Android SDK 的 `apksigner` 和你保留的签名密钥签名后安装。后续更新必须沿用同一密钥。
- 本次交付使用 release 构建（不可调试）并以当前构建环境的开发证书签名，仅交付 APK，不包含私钥。正式渠道发布应使用项目正式签名配置。

单元测试覆盖 URL/来源边界、HTTP 明示允许、真实 HTTP 登录和跨站重定向拒绝、加密随机性与篡改/错误密钥拒绝。JVM HTTP 测试显式启用 `Origin`，因为桌面 JDK 默认限制此请求头；Android 网络栈无此桌面限制。设备验证使用合成服务与口令，不连接真实用户工作区。

该模块不修改 Rust/Node 登录协议或共享桌面行为，因此不需要新增 Rust/Node parity 分歧项。Android 壳版本独立于桌面版。
