# Trae账号管理（trae-cc）

一款基于 Tauri 2 的 Windows 桌面工具，用于管理多个 Trae IDE 账号：本地存储、一键切换（改写 IDE 登录态与机器码）、用量查询与统计图表、机器码管理、每日签到与隐私模式写入。

> **免责声明**：本工具仅供个人学习与技术研究使用。使用者需自行承担全部风险（包括但不限于系统损坏、数据丢失、账号异常等）；本工具可能涉及软件使用协议相关风险，请自行评估；作者不承担任何直接或间接损失责任；严禁商业用途，不得用于绕过软件正当授权机制。继续使用即表示您已理解并同意上述条款。

## 功能特性

- **多账号管理**：本地存储多个 Trae 账号（Token / Cookies / 邮箱密码），支持导入、导出与批量操作
- **一键切换**：改写 Trae IDE 登录态（清理本地状态、写入 `storage.json` 登录信息与 telemetry 标识），并按账号同步机器码
- **用量查询**：查询套餐类型、额度与使用情况，支持统计图表与使用事件记录
- **机器码管理**：查看、重置、设置 Windows MachineGuid，支持将机器码绑定到指定账号
- **每日签到**：手动或开机自动领取每日积分，支持签到冷却与设备标识重置
- **隐私模式**：可选地在切换账号后向 Trae 的 `state.vscdb` 写入隐私模式开关
- **浏览器辅助登录**：内置 webview 登录窗口，自动捕获登录凭据完成账号录入
- **开机自启与静默刷新**：支持以 `--silent` 无头模式在后台刷新账号 Token

## 软件截图

<div align="center">

![Trae IDE 配置](image.png)

![账号管理](image%20copy.png)

![高级切换](image%20copy%202.png)

</div>

## 系统要求

- Windows 10 / 11（仅 Windows 为完整实现）
- 已安装 Trae IDE
- 修改机器码（MachineGuid）的功能需要以管理员权限运行

## 安装

### 下载安装

1. 前往 Releases 页面下载最新版本安装包
2. 运行安装程序
3. 启动「Trae账号管理」

### 从源码构建

前置要求：[Node.js](https://nodejs.org/)（v18+）、[Rust](https://www.rust-lang.org/tools/install) 工具链。

```bash
# 克隆仓库
git clone <your-repo-url>
cd trae-cc

# 安装前端依赖
npm install

# 开发模式运行
npm run tauri dev

# 构建生产版本
npm run tauri build
```

构建默认不产出安装包（`bundle.active = false`），需要时可在 `src-tauri/tauri.conf.json` 中临时开启。

## 使用指南

### 1. 配置 Trae IDE 路径

打开应用后进入「设置」，在 "Trae IDE 路径" 处点击 **自动扫描**，或手动选择 `Trae CN.exe` 的位置。本项目仅适配国内版 Trae CN（数据目录 `%APPDATA%\Trae CN`、API 域名 `api.trae.com.cn`），不支持国际版。

### 2. 添加账号

在「添加账号」弹窗中可选：

- **Token 添加**：粘贴已有账号的 JWT Token（可附带 Cookies）
- **邮箱密码添加**：通过接口登录获取凭据
- **浏览器登录**：在内置登录窗口中完成网页登录，自动捕获凭据

### 3. 切换账号

在账号卡片上点击切换。切换会关闭 Trae IDE、清理其本地登录状态、写入目标账号的登录信息与机器码，然后重新启动 Trae。如在设置中开启了隐私模式，会在启动后自动写入并二次重启生效。

### 4. 用量查询与统计

账号卡片展示当前用量；「统计」页面提供图表化的用量与使用事件视图。Token 过期时会自动尝试用 Cookies 刷新。

### 5. 机器码管理

在设置或账号操作中可查看/重置系统 MachineGuid、将当前机器码绑定到账号（切换该账号时自动恢复对应机器码）。

## 技术栈

| 层 | 技术 |
|---|---|
| 前端 | React 19、TypeScript、Vite 7、recharts、纯 CSS |
| 后端 | Rust（Tauri 2.2）、tokio、reqwest、rusqlite、winreg / windows-sys、warp |
| 插件 | tauri-plugin-opener / dialog / updater / log |

## 项目结构

```
src/            # React 前端（页面、组件、api.ts invoke 封装、共享类型）
src-tauri/      # Rust 后端（账号域 account/、API 客户端 api/、机器码 machine.rs 等）
```

面向开发者的详细架构说明、关键机制与编码约定见 [AGENTS.md](AGENTS.md)。

## 贡献

欢迎通过 Issues 报告 Bug 或提出功能建议，也欢迎提交 Pull Request。提交代码前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md) 与 [AGENTS.md](AGENTS.md)，并遵循 [Conventional Commits](https://www.conventionalcommits.org/) 提交规范。

### 报告问题

1. 先搜索现有 Issues，避免重复
2. 点击 "New Issue"，描述复现步骤、期望与实际行为
3. 附上环境信息（操作系统、应用版本）与相关日志（日志文件位于 `%LOCALAPPDATA%\HHJ\TraeCC\data\logs\app.log`）

## 许可证

本项目采用 MIT 许可证，详见 [LICENSE](LICENSE)。

## 致谢

- [Tauri](https://tauri.app/) — 桌面应用框架
- [React](https://react.dev/) — UI 框架
- [Rust](https://www.rust-lang.org/) — 系统编程语言
