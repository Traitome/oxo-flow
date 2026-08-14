# oxo-flow Web 系统全方位用户测评报告

> 日期：2026-08-14 · 被测版本：后端 v0.11.0（web full-lifecycle，main@c9af6f6 之后）· 前端 bundle 显示 v0.9.2（见 P4-16）
> 方法设计：[2026-08-14-web-evaluation-design.md](../superpowers/specs/2026-08-14-web-evaluation-design.md)
> 证据目录：`/tmp/oxo-eval/`（scripted/、personas/、panel/、screenshots/）

## 1. 执行摘要

**综合评分：可用性 3/5 · 好用 3/5 · 实用 2/5 · 功能完整性 2/5 · 可靠性 2/5（评审团裁决，五维加权约 2.4/5）**

12 名拟真用户（3 个水平 × 5 个生信领域 + 管理员/运维/可靠性专职）完成 12 段旅程与自由探索，产出 99 条原始发现（16×P1 / 34×P2 / 33×P3 / 16×P4），经 5 人独立评审团交叉验证合并。核心判断：

**骨架正确、亮点真实，但"接线层"大量断裂。** 正常路径上，图形化编程（接地工具面板、file 边虚线、30 节点 1.3s 渲染）、知识库（6103 工具带真实版本）、Dockerfile/Singularity 导出、SSE 断连重连、并发与结构化错误均实测可用；但 16 条 P1 全部属于"看起来有、用起来无"或"静默失败"两类——设计文档 §9b 的 9 条功能声明中 **3 条证伪（#3/#4/#9）、2 条存疑（#5/#8）**，P0 阶段声称已修复的 **B1（真实 cancel）与 B5（checkpoint 状态派生）在真实路径上不成立**。

Top 5 问题：
1. **执行真相失守**：cancel 返回成功但不杀进程（子进程照常跑完写产物）；运行中重启把 in-flight run 谎报 failed 且拒绝对其操作——直接违反设计"web 绝不撒谎"底线
2. **状态多源四分五裂**：同一 run 在列表/详情/节点/监控页/诊断页显示 4+ 种互相矛盾的状态（s07 实测 24 次采样 23 次不一致）
3. **平台安全三件套零生效**：限流因 layer 顺序 bug 静默跳过、审计表 0 行、管理员无任何凭证可登录
4. **AI 主路径无兜底**：生成流程后必现超轮次、Accept 按钮永不出现、刷新即丢会话（7 个 persona 见证）
5. **静默失败模式贯穿全站**：验证只显示"23 error(s)"无详情、dry-run 数字失真、样本名拼错仍 Valid、Save 恒报成功

正面：p05（中级 WGS）画布旅程全通并给出全场最高分（可用性/好用/实用/完整性均 4 分）；p12 证实并发 20/20 零 5xx、重启后数据完好、SSE 重连终态不丢——骨架值得保留，修复集中在接线层而非重写。

## 2. 测评方法

- **三层架构**：L1 脚本化客观覆盖（功能矩阵/CLI 对等/并发/错误路径/SSE/状态一致性，7 组脚本，全部可重跑）→ L2 拟真用户探索（12 名 persona，Playwright 真实驱动浏览器，各自旅程 + 自由探索 + 故意犯错）→ L3 独立评审团（UX 专家/资深生信工程师/可靠性工程师/产品经理/对手方，基于全部证据裁决，对手方对全部 P1 做交叉验证）。
- **Persona 矩阵**：3 水平（新手/中级/高级）× 5 领域（RNA-seq/WGS/scRNA/宏基因组/表观）+ 管理员 + 运维 + 可靠性专职 = 12 人。每人五维打分（1–5 分附证据），产出结构化发现卡（复现步骤 + 截图 + severity）。
- **环境**：macOS 本机，`oxo-flow-web --port 3000` personal 模式，前端生产构建；真实 AI（DeepSeek deepseek-v4-pro，Anthropic 兼容端点，key 取自 `ANTHROPIC_AUTH_TOKEN`），每人 ≤3 轮真实对话；真实 run 为微型 system-backend 工作流（echo/sleep）。
- **执行过程说明**：测评期间用户网络两次中断，导致 6 名 persona 与 1 名评审被 watchdog 中断（均已通过收尾指令完成取证，证据完整）；批次 B 起启用增量写卡 + 等待限时防护。评审团剔除证据不足的发现 4 条（含早期探针观察到的"完成写入 2min 延迟"，因未留存量化证据）。

## 3. 维度评分总览

### 3.1 Persona × 维度热力图

| Persona | 可用性 | 好用 | 实用 | 完整性 | 可靠性 |
|---------|:---:|:---:|:---:|:---:|:---:|
| p01 新手·RNA-seq（湿实验博士） | 3 | 3 | 4 | 3 | 3 |
| p02 新手·scRNA（免疫学硕士） | 3 | 3 | 4 | 3 | 3 |
| p03 新手·宏基因组（Excel 用户） | 3 | 3 | 2 | 3 | 3 |
| p04 中级·RNA-seq（分析员） | 3 | 3 | 2 | 3 | 2 |
| p05 中级·WGS（画布用户） | 4 | 4 | 4 | 4 | 3 |
| p06 中级·宏基因组（分析员） | 4 | 3 | 3 | 3 | 2 |
| p07 高级·RNA-seq（迁移评估） | 3 | 3 | 3 | 3 | 3 |
| p08 高级·scRNA（画布上限） | 4 | 3 | 4 | 3 | 3 |
| p09 高级·ChIP-seq（执行控制） | 3 | 2 | 2 | 2 | 2 |
| p10 管理员 | 3 | 3 | 3 | 2 | 2 |
| p11 运维管理员 | 4 | 3 | 3 | 3 | 2 |
| p12 可靠性专职 | 4 | 3 | 3 | 3 | 2 |

### 3.2 分人群平均

- 新手（p01–p03）：可用性 3.0 / 好用 3.0 / 实用 3.3 / 完整性 3.0 / 可靠性 3.0 —— 入口可达，但每个"落地动作"都有静默失效
- 中级（p04–p06）：可用性 3.7 / 好用 3.3 / 实用 3.0 / 完整性 3.3 / 可靠性 2.3 —— 画布路径亮眼，CLI 习惯映射一半失效
- 高级（p07–p09）：可用性 3.3 / 好用 2.7 / 实用 3.0 / 完整性 2.7 / 可靠性 2.7 —— 迁移评估结论：web 是"CLI 的薄前端"而非 CLI 超集
- 管理/运维（p10–p12）：可用性 3.7 / 好用 3.0 / 实用 3.0 / 完整性 2.7 / 可靠性 2.0 —— 平台面承诺与实测差距全场最大

### 3.3 评审团最终裁决

| 维度 | 分数 | 裁决理由（摘要） |
|------|:---:|------|
| 可用性 | **3** | 基本旅程可走通、错误文案与画布图例对新手有真实亮点；但"静默失败"（18 条合并发现中 12 条无反馈路径）系统性地阻断自我纠错 |
| 好用 | **3** | 接地工具面板、file 边虚线、30 节点 1.3s 性能是亮点；版本号不一致、phase 恒 parsing、dry-run 与真 run 不可区分持续侵蚀信任 |
| 实用 | **2** | 知识库/模板/导出等基础件可用，但生信日常三个核心动作（samples 试跑、dry-run 预览、报告验收）全部失守；16S 生态缺失；真实实验室只能当 CLI 的"预览+回放"界面 |
| 功能完整性 | **2** | 9 条声明 #3/#4/#9 证伪、#5/#8 存疑；P0 修复 B1/B5 在真实路径不成立；CLI 13 个生产命令无 web 对应 |
| 可靠性 | **2** | 状态真相、控制真相、可观测性三线失守：同一 run 四种状态词、cancel 不杀进程、重启谎报 failed、限流与审计双双零生效（5 条 P1 全部有源码级根因或脚本量化） |

## 4. 分级问题清单（合并去重后）

### P1 — 阻断核心任务（12 条，全部经对手方交叉验证）

| # | 标题 | 维度 | 复现要点 | 来源 |
|---|------|------|---------|------|
| P1-01 | **cancel 是假信号**：API 返回 cancelled 但真实进程不杀，sleep 照常跑满并写出产物 | 可靠性 | 启动 sleep run → cancel → 18s 内进程仍存活、execution.log 无 cancel 记录；信号只达 CLI 包装进程组，规则子进程各自 `process_group(0)`（core process.rs:856） | p12-02、p09-06 |
| P1-02 | **运行中重启谎报 failed + 孤儿执行**：重启后 in-flight run 被标 failed（finished_at=kill+1s），而 CLI 子进程继续跑完写出产物，web 永久记录 failed 且拒绝 cancel（RUN_NOT_ACTIVE） | 可靠性 | p12 重启实测；孤儿恢复按启动时间（>60s 宽限）判定、不检查记录 pid 存活 | p12-01、p12-06 |
| P1-03 | **状态多源四分五裂**：同一 run 在 list/详情/节点/监控徽章/诊断页显示 4+ 种矛盾状态；运行中 detail 恒 queued、phase 恒 parsing、cancelled 后 detail 永远 queued/pending | 可靠性 | s07-timeline 24 采样 23 次 list=running vs /status=queued；p09-02 四源四词量化 | s07、p09-02、p03-05、p04-03、p12-04/07 |
| P1-04 | **限流完全失效**：rate_limit_middleware 因 layer 顺序拿不到 RateLimiter 而静默跳过；40/150/90+ 突发请求零 429 | 可靠性/安全 | server.rs:451-454 层顺序 bug + rate_limit.rs None-skip；s04/s07/p11 三测零限流 | p11-01、s07 |
| P1-05 | **审计日志零写入**：全部操作后 /api/audit 恒空、audit_logs 表 0 行、logs/audit 停更于 2026-05-25；write_audit_log/log_action 生产代码零调用（唯一写点是 fork_pipeline） | 可靠性/合规 | p10/p11 代码级确认 | p10-01、p11-02、s01 |
| P1-06 | **管理员登录/用户管理整体不可用**：所有凭证 401、sessions 表恒空、建用户 API 永远 401；前端无用户管理/审计页面（client 函数存在但无页面调用） | 功能完整性 | admin/admin、default/default 均 Invalid credentials | p10-02、p07-11 |
| P1-07 | **Samples 语义错接**：run 对话框 Samples 误接 CLI `--sample`（追加）而非 `--samples`（过滤）；填 s1 跑完全量 cohort 并造出幻影样本；'ready'/不存在的 S99 被当字面样本静默执行 | 功能完整性/实用 | executor.rs:131-134 代码确认；p07-01 execution.log 实证 | p04-01、p07-01、p03-06、p09-08 |
| P1-08 | **报告产物不可见**：Output Files 非递归，真实产物（peaks.bed/sam/bam/taxa.tsv 等 8 个文件）在报告页完全不可见；file_types 只认 log/oxoflow；AI Narrative 报"4 output files"失实 | 实用 | p03-04/p09-05 实测；s01 report-ask 独立印证 | p03-04、p09-05、s01 |
| P1-09 | **编辑器保存路径数据不可信**：palette 添加与文本编辑竞态把内容交错成乱码静默入库；Cmd+A 替换后 DAG/验证/Run 仍用旧状态；Save 恒报成功（含 23 错误/成环 TOML） | 功能完整性 | p06-01 两次保存两种乱码；p06-02 所见非所得 | p06-01/02、p10-04、p07-04、p05-01 |
| P1-10 | **AI Chat 生成不交付且无兜底**：复杂请求（scRNA 等）生成完整流程后继续调工具直至 "Agent exceeded max rounds (6)"，pipeline_ready 永不出现、无 Accept 按钮（2/2 复现）；简单请求 2 轮可交付（r5 修正：非必现，根因含 fetch_url 缺 User-Agent 致模型循环）；90s 无文字只出工具卡；刷新丢会话 | 好用/实用 | 7 个 persona 来源；r5 亲自复验并修正范围 | p02-01、p04-05、p03-08、p08-09 等 |
| P1-11 | **验证错误无详情**：UI 只显示 "23 error(s)" 计数徽章，无错误列表/定位；API 层有完整 E002/E004 结构（含行/列/建议）但前端只渲染数字 | 可用性 | p02-02/p07-06 双复现 | p02-02、p07-06、p05-02 |
| P1-12 | **UI 无法增量重跑**：UI 发起的 run 从不携带 pipeline_id → 每次全新 workdir → 改参数重跑全部规则（"4 succeeded, 0 skipped"）；后端能力已存在（API 带 pipeline_id 正确跳过），纯 UI 接线缺口 | 功能完整性/实用 | p09-01 实测；CLI/API 对照 "1 skipped" | p09-01、p09-09 |

### P2 — 严重但可绕行（代表性问题，共 34 条，完整清单见附录 ledger）

- **dry-run 预览失真**（r1 M-02/r2 M-02）：规则名级不展开 wildcard（web 5 规则 vs CLI 13 实例）、samples/targets/max_jobs 在 dry_run 分支被静默丢弃、耗时估计 450s vs 实际 7s；与真实执行计划不一致
- **画布与 TOML 状态不同步**（r1 M-04）：连线成环被拒后修改仍写入 TOML、非法 TOML 时画布保留旧图、错误文案陈旧
- **大 DAG 可读性**（r1 M-06）：16+ 节点自动缩到 zoom 0.29（文字 ~4px）、连线 handle 2.3px 低 zoom 难命中、无 minimap
- **AI 配置不持久化**（r2 M-07）：Settings Save 提示 "✅ Saved" 但 ai_provider_config 表 0 行，重启即回退 Not Configured（声明 #9 证伪）
- **web/CLI 对等差异**（r2 M-08）：web validate 无 rules 数组、duplicate_rule 用 HTTP 400 vs CLI valid=false、lint 警告混入 errors、无系统容量预检
- **CLI 13 个生产命令无 web 对应**（r2 M-09）：resume/init/pull/debug/clean/env/touch/batch/provenance/publish/test/schema/config
- **AI 环境配置静默失效**（r2 M-10）：inspector 改 system 不生效仍走 conda；AI 生成的 rule 级 conda 字段被引擎静默丢弃
- **领域覆盖偏科**（r2 M-12）：QIIME2 及 q2-* 完全缺失（qiime2 搜索仅返回 legacy 1.9.1）；7 个模板全为 RNA-seq/WGS，零 16S/宏基因组模板
- **Save 永远新建行**（r2 M-13）：无原地更新（updatePipeline API 存在但 UI 不调用）；"My Pipelines" 不按用户过滤、100 行含大量重复
- **CSP unsafe-eval 冲突**（r2 M-20/r1 M-14）：每页 2-3 次 EvalError，Dashboard 表达式求值被拦截
- **run 详情/报告面板可达性**（r1 M-05）：埋在 5700px 无分页列表下方，点击不滚动、"📊 Report" 打开的是 Monitor 概览
- **health/system 假数据**（r3 M-07）：uptime_secs 恒 0、资源指标全 0/null，而 /api/metrics 返回真实值（同服务器三端点互相矛盾）

### P3/P4 摘要（33+16 条）

按重复簇归类（r4 统计）：状态一致 11 条、samples/dry-run 语义 7 条、编辑器防呆 9 条、AI 交付 10 条、管理面 9 条。代表性：报告 Q&A 只认英文关键词（中文提问全落兜底模板）、chat 会话不持久化、checkpoint/skipped 对用户不可见、模板/样本表管理缺 UI、undo/redo 不对称（redo 不回填栈、50 步上限）、transform 节点无可视化标记、工具搜索空结果无提示、`?template=` 只认 UUID、**前端版本号 v0.9.2 vs 后端 0.11.0**（4 人独立证实）、模板 DELETE 端点无鉴权、events.jsonl 0 字节死文件。

## 5. 分人群洞察

**新手（p01 湿实验博士 / p02 免疫学硕士 / p03 Excel 用户）**：入口旅程（模板→加载→DAG→验证→run）对鼠标用户整体可达，失败信息与 dry-run 解释可自我纠错（p01-06 正面项）；但每一个"落地动作"都有一层静默失效：AI 交付必现超轮无 pipeline_ready、验证只有计数徽章无定位、dry-run 摘要与真实计划不符、报告埋在 5700px 长列表下方。新手感知 = "界面好看但结果不可达"。系统对新手"过度容错"：Save 在 23 个错误时仍成功、`{samle}` 拼错验证仍 Valid 且静默改变执行语义、不存在的样本 S99 被静默"跑完"——新手没有任何拦截信号，错误成本全部后置到结果阶段。

**中级（p04 分析员 / p05 画布用户 / p06 宏基因组）**：p05 画布旅程是全场最亮的正面项——工具面板搜索真实接地（bwa 0.7.19、samtools 1.23.1、picard 3.4.0、gatk4 4.6.2.0）、连线直觉、file 虚线边与 depends_on 实线语义清楚、inspector 字段全，图形化编程这一差异化卖点在正常路径上成立。但 CLI 习惯映射一半失败：Samples 过滤被误接成追加语义、dry-run 完全不可信、报告里找不到真实产物；中级用户是"被骗得最惨"的人群——他们知道自己想要什么，系统却静默给错东西。

**高级（p07 迁移评估 / p08 画布上限 / p09 执行控制）**：迁移评估证实 web 基础管线可用（粘贴导入字节精确、导出正确、模板/登录页均在），但迁移决策被四个真相缺口拖累：samples 语义错、dry-run 粒度与 CLI 不一致、资源估算字段错位、13 个 CLI 子命令无 web 对应。结论：**web 是"CLI 的薄前端"而非 CLI 超集**，生产团队迁移需等到执行真相层与状态层修复。画布性能达标（30 节点 1.3s、零重叠），瓶颈在可视设计（zoom/文字/undo）。

**管理员与运维（p10/p11/p12）**：平台面承诺（ROADMAP Phase 10：多租户、审计、资源感知仪表盘）与实测差距全场最大：审计 API 与表都在但零写入、管理员登录所有凭证失败、AI 配置"✅ Saved"实为内存级假成功。运维信任基线未建立：限流从未生效、health 假数据。管理面整体是"看起来有、用起来无"。

## 6. 功能声明核验表（对照设计文档 §2.3/§9b）

| 声明 | 裁决 | 证据 |
|------|------|------|
| #1 画布 palette 接地工具 + inspector + file edge 虚线 | ✅ 成立 | s01 canvas pass；p06 实测 metaPhlAn 4.2.4/kraken2 2.17.1；p08 file 边虚线正确 |
| #2 Chat 真实 tool-calling + grounded 工具卡 + typed SSE | ✅ 成立（带瑕疵） | p07-12 短问题实测 6+ 次 lookup 调用并给出带 TOML 的接地答案；但生成交付链（pipeline_ready）失败，见 P1-10 |
| #3 run 对话框 samples/targets/keep-going/max_jobs 真实生效 | ❌ **证伪** | samples 错接 `--sample` 追加语义（executor.rs:131-134）；dry-run 分支丢弃全部选项 |
| #4 真实 cancel/pause/resume（真正信号进程） | ❌ **证伪** | P1-01：cancel 后进程存活并写产物；信号只达 CLI 包装组，规则子进程独立 process_group |
| #5 报告 Q&A + 可视化来自真实 run 数据 | ⚠️ 存疑 | 数据部分真实（B7 罐头模板已修），但 Output Files 非递归致产物不可见、Q&A 英文关键词分支中文提问全落兜底 |
| #6 模板 `?template=` 加载进编辑器 | ✅ 成立 | p10-06 UUID 加载正常；按名字构造 URL 404（P3） |
| #7 saved-pipelines 页 list/open/导出/delete | ✅ 成立（带瑕疵） | p07 实测导出 Dockerfile/Singularity 正确；但 Save 永远新建行、无原地更新 |
| #8 login（token→oxo_token）+ header 显示用户 | ⚠️ 存疑 | 机制存在（login 页/auth client/结构化 401），但 12 个 persona 无一成功登录（所有凭证 401） |
| #9 AI config 存 DB、重启恢复 | ❌ **证伪** | p10/p11/p12 三方确认：Save 只改内存，表 0 行，重启后 is_configured=false |
| P0-B1 cancel/pause/resume 真正信号进程 | ❌ **证伪** | 同 #4（killpg 实现存在但对象只是 CLI 包装组） |
| P0-B2 状态词汇统一 completed | ✅ 成立 | runs 行终态词汇正确（executor 写 completed） |
| P0-B3 audit_logs schema 统一 | ✅ schema 成立 / ❌ 功能死亡 | 表存在可查询，但写入链路生产零调用（P1-05） |
| P0-B4 insert_run 12 列全量写入 | ✅ 成立 | 100+ run 完整落库（pid/status/finished_at/workdir 齐全） |
| P0-B5 checkpoint 派生节点状态 | ❌ **证伪（通配符路径）** | 非通配符流程正确；`{sample}` 流程展开实例名（fastqc_auto-discovered_s1）与规则名不匹配 → DAG 全 pending、/status 恒 queued |
| P0-B9 save_pipeline owner 按会话用户解析 | ✅ 成立 | personal 模式归 default 用户，无越权证据 |
| P0-B10 get_ai_config_effective 读取真实配置 | ✅ 成立（带瑕疵） | 返回真实 provider=deepseek；但 Settings 下拉选中态显示 openai、配置不落库 |

## 7. 改进建议（按优先级，评审团排序）

1. **R-01 执行真相层重做**（p12-01/02/06 等，P1×5）：进程 registry 持久化到 DB（run_id→pgid+spawn 时间），启动时扫描恢复——真实进程已退出→按退出码归因；仍存活→标记 running 并重挂控制；cancel 信号穿透到规则子进程组。成本中-高。
2. **R-02 状态单一事实源**（11 条发现，重复率最高簇）：list/detail/DAG/SSE 全部消费同一派生状态（checkpoint 规则完成计数加权，完成前不报 queued）；删除或重定义 phase 伪字段。成本中，修一处省多处。
3. **R-03 UI run 携带 pipeline_id**（p09-01，P1）：Run 对话框默认绑定当前 pipeline_id → 启用增量重跑；DAG 页展示 skipped/up-to-date 词汇与 checkpoint 状态；重跑时高亮本次实际执行的规则。成本低（纯前端接线）。
4. **R-04 run 对话框语义修正**（7 条发现）：Samples 改接 `--samples` 过滤语义；dry-run 与 run 共享同一 CLI 参数构建函数；dry-run 预览按样本实例级别展示并对齐 CLI 输出；不存在的样本名给出警告而非幻影执行。成本低-中。
5. **R-05 平台安全三件套**（P1×4）：修 rate_limit layer 顺序 bug；审计写入挂到统一 mutation middleware（一个写入点覆盖所有 handler）；管理员密码配置 + sessions 初始化。成本中。
6. **R-06 验证错误可视化 + 编辑器防呆**（9 条发现）：错误列表面板 + 行内定位（diag 引擎 30+ 模式已存在）；Save 前强制校验 gate（未知字段拒绝而非静默忽略）；服务端 DAG 编辑原子化——被拒修改回滚到编辑前 TOML。成本中。
7. **R-07 AI 交付链路稳定化**（P1×2+P2×4）：超轮时给出降级路径（简化流程/直接交付已生成 TOML）；environment 字段真实回填（inspector 改 system 必须删 conda 段）；配置/会话持久化。成本中。
8. **R-08 报告与结果浏览器**（P2×4+P1×1）：Output Files 改递归文件树（或后端生成文件清单）；/runs/:id 独立路由页 + 自动滚动 + 正确 tab 锚点（点 Report 必须落在报告）；runs 列表分页。成本低-中。
9. **R-09 画布一致性闭环**（P2×6+P3×3）：TOML/画布/验证三方状态机统一（编辑失败回滚+提示）；undo 栈 redo 压栈修正；transform 可视化标记；大图 minimap + 最小字号下限。成本中。
10. **R-10 版本纪律 + 领域补齐**（P3×7）：CI 前端构建注入版本号 + 启动时前后端版本一致性检查；新增 3-5 个 16S/宏基因组模板 + QIIME2/q2-* 工具补充。成本低。

**r4 关键提示**：被测前端 bundle 显示 v0.9.2 而后端 0.11.0（4 人证实），§2.2/§9b"已修复"项与实测的冲突，须先在版本一致的构建上复测后再排修复。正面项（p05 画布全通、p08 性能、p12 基础架构稳）证明骨架正确，缺陷集中在接线层，无需重写。

## 8. 对手方交叉验证（r5）

对手方对全部 P1 与关键 P2 亲自复验（非转述），共 49 条裁决：**36 verified / 13 downgraded / 0 refuted**。关键裁决：

- **P1-02 重启谎报 failed：VERIFIED（最硬证据）**——DB 行 failed@05:40:46，而产物 out/p12_done.txt mtime 13:41:23（failed 标记后 37 秒写出）+ execution.log "Done: 1 succeeded"；db.rs:236 孤儿恢复是无条件盲 UPDATE，不查进程存活
- **P1-01 cancel 是假的：VERIFIED**——out/c.txt 在 cancel 确认后 56 秒照常写出；根因 core/executor/process.rs:856：每个规则 `sh -c` 自建进程组，web SIGTERM 杀不到规则子进程
- **P1-04 限流失效：VERIFIED**——实测 120 同 IP 请求零 429；server.rs 层序 rate_limit_middleware 在 Extension 外层，拿不到 RateLimiter 直接跳过
- **P1-10 AI 超轮：VERIFIED 但修正范围**——复杂 scRNA 请求复现 "Agent exceeded max rounds (6)"（10 次 tool_call）；但简单请求 2 轮即出 pipeline_ready，故「必现」不成立；根因含 fetch_url 缺 User-Agent 致模型循环
- **DOWNGRADED 代表**：p12-05（`[[rulz]]` 一行式实为 400，载荷与发现不符）；p09-07 dry-run 404 子项未复现；p06-01/02 前端竞态族、p08 与 p11-06 浏览器侧发现（网络中断未交叉验证，标 downgraded-to-doubtful，保留原级但附注）
- 其余 P1（p02-02、p03-01/04、p04-03、p05-01、p07-01、p09-01/04、p10-01/02/04/05、p11-02/03/04/05、p12-03/04）全部 verified，多为 DB+API+源码三通道印证

## 9. 局限与范围说明

- **环境后端**：conda/docker/mamba/singularity 后端未实测（本机未装对应工具）；仅 system backend 微型工作流（echo/sleep）。真实生信工具（fastqc 等）未安装，工具执行层面未验证。
- **AI 评估**：真实调用受 DeepSeek 配额与网络影响；"AI exceeded max rounds"与模型行为/网络的相关性由对手方裁决。token 级流式未实现属文档已声明偏差，不计入新发现。
- **多用户/多租户模式**：仅 personal 模式实测；OAuth 全流程、企业部署形态未覆盖。
- **单机环境**：并发与性能数字只反映单机；生产级负载、跨主机调度（HPC 模式）未测。
- **测评者偏差**：persona 为 LLM 扮演，主观评分代表"拟真用户视角"，与真实人类用户可能有差异；对手方评审已对全部 P1 与关键 P2 做交叉验证，剔除证据不足项 4 条。
- **已知环境噪声**（非产品缺陷）：本机存在此前测试会话遗留的 ~40 个 oxo-flow-web 僵尸进程（随机高端口监听）。

## 10. 附录：证据索引

- L1 脚本：`/tmp/oxo-eval/scripted/s01-feature-matrix.json`（36/44，8 项为断言口径修正）、`s02-cli-parity.json`（4/7，3 项真实对等差异）、`s03-concurrency.json`（5/5）、`s04-error-paths.json`（17/17）、`s05-sse-reconnect.json`（2/3）、`s07-status-consistency.json`（3/6，逐样本时间线见 `s07-timeline.json`）
- Persona 发现卡：`/tmp/oxo-eval/personas/p01–p12/card.json`（99 条原始发现）；合并索引 `/tmp/oxo-eval/findings-ledger.json`
- 评审团裁决：`/tmp/oxo-eval/panel/r1-ux.json`、`r2-senior-bioinfo.json`、`r3-reliability.json`、`r4-pm.json`、`r5-devils-advocate.json`
- 截图：`/tmp/oxo-eval/screenshots/l1/`（8 页 + 画布操作）、`/tmp/oxo-eval/personas/pNN/screenshots/`（12 人共 250+ 张）
- 服务器日志：重启前实例日志见会话任务输出（含 executor 行）；重启后 `/tmp/oxo-eval/server-restart.log`
