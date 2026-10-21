# 多设备播放与 HomePod 立体声组合

Airloom 当前支持向多个独立接收端发送音频。**Apple 原生立体声组合的创建、拆散和左右声道切换尚未实现；向已有原生组合投放也尚未经过本项目实机验证。**

## 能力边界

| 功能 | 含义 | 当前状态 |
| --- | --- | --- |
| 多设备播放 | 将同一份完整双声道音频发送给多个接收端 | 已有独立会话与分发实现；不承诺样本级同步或实测声学同步 |
| 软件左右声道分配 | 一台接收左声道，另一台接收右声道，由发送端保证同步 | 尚未实现；仅拆分 PCM 不能保证两台同步发声 |
| 向已有 Apple 立体声组合投放 | 识别已经在“家庭”中组成的一对 HomePod，并正确处理其成员和时钟 | 未验证，不能作为已支持的兼容性声明 |
| 在 Airloom 中管理 Apple 原生组合 | 创建、拆散组合，或交换左右声道 | 尚未实现 |

Apple 将控制中心里的多房间选择与立体声组合分别介绍：前者选择在哪些设备播放，后者让两台 HomePod 分别承担左右声道。参见 [Apple 对两项功能的说明](https://www.apple.com/newsroom/2018/05/ios-11-4-brings-stereo-pairs-and-multi-room-audio-with-airplay-2/)。

## 在 Apple“家庭”中建立组合

按 [Apple 官方 HomePod 使用手册](https://support.apple.com/zh-cn/guide/homepod/apd1ed62a52a/homepod)：

1. 使用两台**相同机型**的 HomePod，例如两台 HomePod mini、两台 HomePod（第 1 代）或两台 HomePod（第 2 代）。
2. 在“家庭”中将两台设备分配至**同一房间**。
3. 在 iPhone 或 iPad 打开“家庭”，选择一台 HomePod，进入设置，选择“创建立体声组合”。
4. 建立后，可在组合的“音频设置”中切换左右声道；“取消配件组合”用于拆散。

这些步骤建立的是 Apple 设备上的组合，**不表示 Airloom 已验证对该组合的投放**。不能把一台 HomePod 与一台 HomePod mini，或不同代的 HomePod，按上述官方方式组成同一立体声对。

## 当前实现缺口

- [设备发现](../crates/airloom-airplay/src/discovery.rs)按物理设备标识合并 AirPlay/RAOP 记录，保留 TXT 元数据，但没有完整的组合对象、成员关系与组合选择流程。
- [接收端诊断](../crates/airloom-airplay/src/diagnostics.rs)记录组长、立体声和父组提示。这些提示用于排查，不能单独证明成员关系、左右声道或正确的播放目标。
- [会话建立](../crates/airloom-airplay/src/sender.rs)仍按独立接收端协商。当前多选标志、时钟分支和 SETUP 字段不构成原生组管理实现；也不能仅修改一个标志就声明组合支持。
- [音频分发](../crates/airloom-core/src/engine.rs)向各 sender 提交同一份完整双声道 PCM。每个 sender 独立建立首包时间锚点，目前没有组级“同一个样本在共同指定时刻发声”的启动契约。

公开协议研究说明 `gid`、`pgid` 等发现字段会随播放状态变化，因此不能只凭一个分组字段、相同名称或两台被同时选中，就推断原生立体声关系。参见 [OpenAirPlay 服务发现研究](https://openairplay.github.io/airplay-spec/service_discovery.html)。[OwnTone 的组合调查](https://github.com/owntone/owntone-server/issues/1413)也分别讨论了界面组合、成员传输及 Apple TV 家庭影院路径；其中的特定固件观察不作为 Airloom 的通用保证。

## 后续需要完成的验证

优先研究已有原生组合的发现与投放，再研究 Windows 内的组合管理。需要记录组合前后、空闲/播放、独立组合及 Apple TV 默认输出等状态，核实成员识别、目标选择、认证和时钟行为，不能预先假定只连组长或同时连接全部成员一定正确。

若增加软件左右声道模式，还需实现共同样本序号、预定播放时刻、各接收端延迟处理、组启动屏障、重连对齐与漂移处理。共享采集或共享 PTP 监听器不足以代替这些约束。可参考 [Music Assistant 的原生组合与软件声道分配说明](https://www.music-assistant.io/faq/how-to/#create-a-stereo-pair)及[发送端时钟与组启动设计](https://github.com/music-assistant/airplay-cli/blob/main/DESIGN.md)。

验收必须使用真实两台设备：左右声道测试音、同时起播、长时间漂移、单台断线/重连、密码与音量控制，并记录机型、固件、网络和可听结果。当前自动化测试不替代这些实机结果。
