# teal-browser

受管会话和页面在启动进程、创建 CDP target 前预占配额；创建失败、取消或关闭会释放
名额。配额统计包含创建中的资源，保证并发请求不会超卖 8 个会话或每会话 64 个页面。
`/json/new` 已成功但导航或调用被取消时，后台清理守卫会关闭尚未登记的 CDP target，避免页面资源和配额状态分离。

受管 Chromium 的 CDP 能力：管理器固定可执行文件，每个会话使用独立 profile，只连接自身创建的 endpoint，不附加用户日常浏览器，也不提供任意 JavaScript 执行。DOM 与可访问性快照返回带 generation 的节点引用，交互后引用立即失效。

跨层位置与依赖方向见 `teal-core` README。

启动、导航、点击、填写和关闭均在动作前写入审计意图；独立嵌入宿主用 `BrowserManager::with_audit_context` 提供持久接收器。审计事件不包含 URL、页面文本或填写值。

每个页面在导航前启用 CDP Fetch 请求拦截。`browser.open` 对入口 URL 取得独立网络授权；重定向和子资源只在会话已登记精确规范 URL 时放行，未获准请求会被阻止并报告目标。调用方对报告的目标另行批准 `browser.allow_network(session, url)` 后重新打开页面。许可随会话关闭消失；请求放行前写受控审计意图，交给 Chromium 后写结果，账本不保存 URL。CDP 拦截连接中断会使页面操作失败关闭。

页面 CDP 会话在导航前递归自动附加相关 popup、独立 iframe 和 worker target；新 target 暂停至对应会话启用 Fetch 拦截后才运行。子会话请求复用同一精确 URL 许可与审计边界。拦截连接或设置失败会终止受管浏览器进程，避免剩余 target 脱离拦截继续联网。
若页面创建失败时已出现子 target，关闭页面拦截也会终止该会话；调用方须新建会话并重新取得所需精确 URL 许可。
