# AGENTS.md

## Commit 规范

- 标题精简（一句话概括，不带过细细节），具体的改动说明放到 commit body 中。

## 前端完整性守护（强制）

所有涉及 `src-tauri/dist/` 的改动，提交时会自动经过守护脚本检查
（`.git/hooks/pre-commit` → `node tools/check.mjs`，每次提交全量运行，<1s）。

**四类校验**：
1. HTML 引用与磁盘文件双向一致（含孤立文件检测）
2. 跨文件调用审计：调用的标识符必有声明
3. 全量 JS `node --check` 语法机检
4. BOM 扫描（CSS/JS/HTML 禁止 UTF-8 BOM）

**防护边界**：结构完整性闸门。能拦引用缺失/孤立文件/未定义调用/语法错误/BOM；
拦不住 CSS 语义错误、合法语法下的逻辑 bug、运行时行为问题——这些仍需构建后人工回归。

**例外通道**：
- `git commit --no-verify` 可跳过钩子，仅限明知未完成的 WIP 中间提交
- 提交前可随时手动自检：`node tools/check.mjs`

**钩子重装**（`.git/hooks/` 不随仓库走，重新克隆后执行）：

```bash
cp tools/pre-commit .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit
```

**维护注意**：新增页面/目录需同步更新 `tools/check.mjs` 的 PAGES 数组；
若标识符审计出现误报，优先扩展 check.mjs 的声明提取规则，而非绕过钩子。

## 前端架构备忘

- 结构：popup/settings 双页体系，脚本"分区在前、入口最后"，命名镜像
  （`popup-{devices,audio}.js ↔ settings-{devices,audio}.js`），全部 JS 带标准头注释
- 已否决路线：方案乙 ESM 迁移（触发重启条件：前端规模翻倍 / 多人协作 /
  config 共享实际出 bug；届时可先考虑 config 抽为经典脚本单例的廉价中间路线）
- 材质系统收敛（删除 settings-general 回调手动三件套）暂缓，
  下次因其他原因动材质代码时顺手做并实测闪烁