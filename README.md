# Airloom

**让 Windows 的声音，流向 HomePod。**

一个从零实现的 Windows 原生音频投放应用：WinUI 3 界面、Rust 音频核心，以及 Windows 自带的端点和进程音频采集。可投放系统声音，也可只选择一个应用；无需安装虚拟声卡驱动。

> **0.4.2**：接入全新 WinUI 原生界面，使用 Windows 11 Mica、Fluent 控件、系统强调色和原生动效；清理立体声旧提示，保留先左后右测试音。应用与设备继续通过事件通知更新；版本末位递增至 `.10`，再进中间位。

[![Build and verify](https://github.com/chummumm/airloom/actions/workflows/ci.yml/badge.svg)](https://github.com/chummumm/airloom/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-626a73.svg)](LICENSE)

## 界面

以下为 0.4.2 的实际 WinUI 界面截图，来自 [CI 39701717193](https://github.com/chummumm/airloom/actions/runs/39701717193)／`481d3ee`。设备、应用和信号为界面测试数据。

![Airloom 0.4.2 原生立体声组合界面，设备和信号为界面测试数据](https://github.com/chummumm/airloom/releases/download/v0.4.2/fixture-native-stereo-home-light.png)

![Airloom 0.4.2 原生深色设置页与组合可见性，设备为界面测试数据](https://github.com/chummumm/airloom/releases/download/v0.4.2/fixture-native-stereo-settings-dark.png)

截图来自实际 WinUI 3 程序。`fixture-*` 使用独立临时目录中的 CI 专用替代 DLL，提供模拟扬声器、信号、应用选择与密码挑战，用于检查原生界面的完整状态；这些数据和替代 DLL 不进入安装包，也不作为实机音频证明。

发布程序本体另有[浅色空状态](https://github.com/chummumm/airloom/releases/download/v0.4.2/native-home-light.png)、[深色空状态](https://github.com/chummumm/airloom/releases/download/v0.4.2/native-home-dark.png)及[设置页](https://github.com/chummumm/airloom/releases/download/v0.4.2/native-settings-light.png)截图。Windows CI 从便携 ZIP 启动程序，验证 880／1100／1600 像素窗口、固定播放栏、音量实际拖动、深浅主题、设置导航、诊断复制与真实 GitHub 更新检查。隔离 fixture 验证应用选择、范围切换、事件更新与手动刷新保留选择、下拉框稳定性、组合卡及成员状态、隐藏恢复，以及真实鼠标操作主按钮。UIA 焦点检查仅证明逻辑焦点，不声称已验证可见键盘焦点框。仓库 `docs/evidence` 中原有应用模式截图保留为 0.4.0 历史证据。

## 体验

- **原生界面**：WinUI 3 / C++/WinRT，Windows 11 Mica 背景、Fluent 卡片、系统强调色、自适应扬声器列表与固定底部播放栏；支持深浅主题、系统高对比度和减少动画偏好。
- **原生按钮与动效**：使用 WinUI 原生模板的焦点、悬停／按下／禁用反馈，加入遵从系统偏好的状态、选择和页面过渡。播放栏和音量数字槽位继续保持稳定。
- **实时展示**：由真实 PCM 驱动的 64 段峰值包络、左右声道电平和传输指标；固定 −60 至 0 dB 显示标尺，没有音频时显示平线。
- **免驱系统声音**：采集系统默认或指定的现有输出设备，通过局域网向 AirPlay 接收端发送。
- **事件自动更新**：应用会话、进程退出和播放设备变化触发来源更新；设备发现持续接收 DNS-SD 事件。没有应用层定时补查，现有刷新按钮用于主动重扫。
- **已有原生立体声组**：识别在 Apple“家庭”中已经建立的 HomePod 组合，整组连接、音量及停止；两台准备好后共用起播时间线。兼容性待实机验证，参见下方说明。
- **指定应用**：通过 Windows 原生进程 loopback 采集所选进程及其子进程。应用退出后不会自动转播其他应用或系统声音；同一浏览器实例的多个标签页可能一起采集。
- **多设备投放**：一次采集，同一份音频发往多个独立连接。可在播放中选择、取消或重试单台设备；一台等待密码或连接失败，不会停止其他设备。
- **密码与配对**：区分设备密码、配对码、长期配对凭据；认证失败可以重新输入。记住凭据需要主动勾选，存入 Windows 当前用户的凭据管理器。
- **独立设置页**：优先显示更新和连接诊断；管理主题、波形、登录启动和设备可见性。隐藏选择会保留，离线设备也可恢复显示。
- **应用内更新**：检查 GitHub 最新正式版本，下载并校验 SHA-256，退出后安装并重新打开。成功、取消或失败后的包会清理；中断和占用残留在后续启动时重试。
- **托盘控制**：单击打开窗口，右键控制；托盘退出真正结束后台。登录自启可选。
- **本机静音**：仅系统声音模式可用，默认关闭；停止时只恢复仍由当前会话拥有的变更，不覆盖用户后来手动修改的状态。应用模式保留本机播放。

## 下载与运行

正式安装包以 [GitHub Latest](https://github.com/chummumm/airloom/releases/latest) 为准。0.4.2 已通过完整构建，以下为本版发布资产。

**从 0.1.x / 0.2.0 升级，请先右键系统托盘中的 Airloom →“退出 Airloom”，再运行新安装包。** 关闭旧版窗口只会隐藏到托盘；旧版没有新的安装退出协议，新安装器会提示先退出。0.2.1 及更新版本可使用应用内更新和新的进程退出流程。详见[0.4.2 版本说明](docs/releases/0.4.2.md)。

| 文件 | 用途 |
| --- | --- |
| [Airloom-0.4.2-setup-x64.exe](https://github.com/chummumm/airloom/releases/download/v0.4.2/Airloom-0.4.2-setup-x64.exe) | 按当前用户安装 |
| [airloom-windows-x64-portable.zip](https://github.com/chummumm/airloom/releases/download/v0.4.2/airloom-windows-x64-portable.zip) | 完整解压到新目录后运行 |
| [SHA256SUMS.txt](https://github.com/chummumm/airloom/releases/download/v0.4.2/SHA256SUMS.txt) | 两个下载文件的完整性校验值 |

0.4.2 来自[全部检查通过的构建](https://github.com/chummumm/airloom/actions/runs/39701717193)，源码提交为 `481d3ee17a06b54c0cdc4ce483716aea9cbd8fa6`。Linux 和 Windows 测试、真实进程采集与来源通知 API 生命周期、原生应用与打包、界面检查和独立更新生命周期均通过。开发构建可在 [Actions](https://github.com/chummumm/airloom/actions/workflows/ci.yml) 的 `airloom-windows-x64` Artifact 中获取。

要求 Windows 11 x64（build 22000 或更高）。电脑与接收端需处于可以互相发现和通信的局域网。解压便携 ZIP 后运行 `Airloom.exe`，或运行安装包。不要只复制 EXE：原生应用和 Windows App SDK 的自包含文件需要放在一起。

### 投放系统声音

1. 顶部声音来源类型保持 **系统声音**，选择电脑正在播放声音的输出设备。
2. 在扬声器列表选择一台或多台接收端；必要时重新发现。
3. 点击 **开始投放**，按提示输入设备密码或配对码。
4. 如需减少本机播放，可尝试 **电脑静音**，并确认接收端仍有声音；该选项作用于所选 Windows 输出上的所有应用。

### 只投放一个应用

1. 顶部声音来源类型选择 **指定应用**。
2. 让目标应用播放一段声音，应用会通过事件通知自动出现；必要时可点 **刷新列表**。
3. 选择要投放的应用；名称后面的进程 ID 用于区分运行实例。
4. 选择扬声器并点 **开始投放**；设备密码流程与系统声音模式相同。

应用暂停后可以继续保持选择。这里选择的是进程及其子进程，不是单独浏览器标签页；同一浏览器实例的多个标签页可能一起采集。应用将声音交给其他无关进程、服务或受保护播放路径的情况仍需单独验证。

切换 **系统声音／指定应用** 会停止当前投放并保留扬声器选择；确认新声源后重新开始。应用退出后不会自动选择其他应用或系统声音，重新启动的实例需要重新选择。应用模式下 **电脑静音** 不可用，本机播放继续，应用原本的输出路由不变。

### 密码、诊断与更新

程序不需要 Apple Account 密码。Windows 声音输出列表不会新增“Airloom 声卡”；投放设备在本应用中选择。

如果波形正常但音响无声，可先按“试播 3 秒”发送独立测试音，然后保持连接，进入设置 → 连接诊断 →“复制诊断信息”。反馈时附上这段文本，便于区分采集、时钟同步、发送和接收端响应问题；它不包含设备名称、网络地址、标识、密码或密钥。波形与发送包数不能单独证明接收端已经播放。

在设置中点击“检查更新”可获取后续正式版本。应用内更新使用安装程序更新当前运行目录，清理仅限 Airloom 自己的更新缓存；手动下载到“下载”文件夹的安装包不在清理范围内。详见[更新设计与验证](docs/updates.md)。

## 多设备与 HomePod 立体声

当前多设备功能向每个独立接收端发送完整双声道音频。它不等于 Apple 原生左右声道组合，也不代表已经验证多音响同步效果。

0.4.1 实现路线 A 的试验性兼容：识别已有 HomePod 原生组合，显示一个输出，等待两台就绪后发送完整双声道；共同时间线和成员故障整组处理由发送器负责。选择组合后可点“测试左右声道”，先左后右共 3 秒。**真实 HomePod 原生组合播放、声学同步和不同固件兼容性仍待实机验证。**

Airloom 不创建、拆散 Apple“家庭”组合，不修改其左右设置，也没有软件左右声道分流。Apple TV 家庭影院绑定组合另列待验证场景。详见[立体声支持边界](docs/stereo.md)。

## 已实现的代码与验证边界

| 项目 | 当前范围 |
| --- | --- |
| 原生窗口、按钮、设置、选择、认证和托盘 | 最终便携包的启动、三尺寸布局、音量实际拖动、按钮按下、应用选择、主题及密码交互均通过；最终应用模式、设置、窄窗口和按钮截图已复核 |
| 系统声音 | 已有端点的 WASAPI 共享模式 loopback；格式转换、双声道重采样、有界缓冲 |
| 单独应用采集 | Windows 11 的进程树 loopback；必须明确选择，目标退出不自动切换。真实 Windows API 激活、身份、收到的静音样本、拒绝本机静音、取消和退出检查已通过；未完成两应用同时发声的实机隔离验证 |
| AirPlay | 设备发现、独立认证/会话/音频发送实现，详见协议文档 |
| 密码保护 | 有协议与 UI 实现；需要逐型号、固件和访问策略实测 |
| 多设备 | 独立发送队列、认证与恢复，共用一次采集；自动测试验证分发完整性和故障隔离，真实多音响效果待测 |
| 更新与包清理 | 独立校验拒绝、成功/取消、重启与缓存清理检查通过；真实 WinUI 进程的安装退出、托盘清理及 Restart Manager 查询/取消/确认退出检查通过 |
| 实际播放延迟 | 尚未测量，不把接收缓冲设置当作端到端实测值 |
| HomePod 原生组合 | 已有组合识别及组级发送为试验性实现；协议测试覆盖共同时间线、双侧完整音频及整组失败，真实 HomePod 仍待验证；创建、拆散和左右交换未实现 |
| 多房间同步 | 未验证，不把共享采集当作同步播放证明 |
| 真实 HomePod 实机结果 | 用户已反馈 0.2.1 在其 HomePod 上恢复出声；其他型号、固件、立体声组合及多设备同步仍需逐项验证 |

验证分为独立层次：协议和音频算法测试覆盖加密会话、非零 PCM、时钟协商、多设备分发与故障隔离；Linux 另通过 FFmpeg 独立 ALAC 解码；Windows 通过真实进程 loopback API 及来源通知注册、显式刷新和关闭检查；原生 UI fixture 检查选择、自动来源变化、原生组合、按钮、主题和密码；更新 fixture 检查退出、安装与包清理。真实播放应用的会话事件交付和 HomePod 双机播放仍需实机验证。完整结果见上述 CI。

密码弹窗与单设备取消通过 UIA 交互检查。本次 `fixture-auth-dark.png` 捕获的是主页，不用来证明密码框外观。高对比度、空应用列表及应用退出状态没有本轮视觉验证声明。详见[设计与验证记录](docs/design.md)。

进程 API 检查使用一个由测试创建、没有播放声音的进程；收到的样本必须为有限值且严格为零。它没有同时播放两个应用，也没有连接真实 HomePod。应用模式的声音隔离、浏览器子进程行为和实际接收端听音仍须实机检查。原生更新检查也不等于已经在线覆盖安装了一台用户电脑。截图和发送包数均不能代替这些实机结果。

WASAPI 采集不会自动改变应用的本地输出路由。独占模式、受保护内容、声卡特有音效和静音位置可能影响采集。没有声卡驱动签名问题，但公开下载的预览应用/安装包仍可能出现系统的发布者或信誉提示。

## 构建

### Rust 核心

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
```

非 Windows 系统可以运行协议和音频算法测试；实际 Windows 音频采集与凭据存储会明确返回不支持。

### Windows 完整应用

安装 Visual Studio 2022 或 2026 的 C++ 桌面开发与 WinUI/C++ 工具组件、Windows SDK 10.0.26100.0、Rust MSVC 工具链、NuGet CLI 和 Python 3（用于收集依赖许可证）。工程会根据 Visual Studio 版本选择 v143 或 v145 工具集。在开发者 PowerShell 中执行：

```powershell
./scripts/build-windows.ps1 -Configuration Release
./scripts/package-windows.ps1
```

第二步需要 Inno Setup 6。输出在 `dist/`。完整步骤也可参考 [Windows CI](.github/workflows/ci.yml)。

## 结构

| 目录 | 内容 |
| --- | --- |
| `native/Airloom` | WinUI 3 / C++/WinRT 原生应用 |
| `native/Updater` | 独立更新助手、下载校验与缓存清理 |
| `crates/airloom-core` | 用户操作、连接状态、会话调度、C ABI |
| `crates/airloom-capture` | Windows 音频采集和凭据存储 |
| `crates/airloom-airplay` | 独立 AirPlay 发现、认证和发送器 |
| `scripts`、`installer` | 构建、检查、打包 |
| `docs` | 设计、协议、兼容性边界 |

参见 [架构](docs/architecture.md)、[界面设计](docs/design.md)、[Windows 音频](docs/windows-audio.md)、[协议](docs/protocol.md)、[贡献指南](CONTRIBUTING.md)。

## 开源

Airloom 原创代码使用 [MIT License](LICENSE)。依赖保留各自许可证，见 [第三方声明](THIRD_PARTY_NOTICES.md)。项目不包含其他投放软件的应用代码分支。HomePod 和 AirPlay 是 Apple 的商标；本项目与 Apple 无隶属关系。
