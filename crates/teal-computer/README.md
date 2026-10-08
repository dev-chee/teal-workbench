# teal-computer

平台中立的桌面窗口、可访问性快照和交互结果。Windows 实现默认使用 UI Automation 的 Invoke 与 Value pattern，密码元素和过期 generation 失败关闭；原始点击与按键是单独能力，不作为语义操作的隐式回退。

跨层位置与依赖方向见 `teal-core` README。

Invoke、Value、原始点击和按键经过持久审计包装；嵌入宿主使用 `platform_backend_with_audit` 注入接收器。审计事件不包含窗口标题、元素值或按键内容。

同一窗口的快照和动作按窗口串行；动作完成预检后、首次 UI Automation 输入前即消费代次。输入调用开始后的失败按效果未知返回并记入 Unknown 审计结果，调用方须重新观察窗口后决定后续动作，不可用旧代次自动重放。
