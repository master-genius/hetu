---
name: glass-bg-noise-water-stain
description: Canvas random noise + CSS blur causes statistical clustering ("water stains") in transparent terminal backgrounds — replace with deterministic text dots to eliminate the effect.
source: auto-skill
extracted_at: '2026-07-08T17:58:01.995Z'
---

# 毛玻璃背景 canvas 噪声水渍问题

## 症状

开启毛玻璃（backdrop-filter blur）后，终端背景出现不规则的亮/暗斑块，
形似"水渍"。在暗色和亮色主题下都可能出现，大面积背景下尤其明显。

## 根因

**数学性质，非代码 bug。** 均匀白噪声（逐像素 `Math.random()`）经高斯模糊后，
局部均值存在统计涨落——某些 blur 半径区域内亮像素偏多，另一些偏少，
模糊后形成肉眼可辨的斑块。

三层叠加效应：
1. **统计聚集**：随机噪声 + blur 的固有性质，无法通过调参数消除
2. **平铺接缝**：canvas 256×256 以 128px 平铺，tile 边界处亮度均值不一致
3. **稀疏亮点扩散**：低 alpha（如 10/255 ≈ 4%）下，少数稍亮像素经 30px 模糊
   扩散为孤立光斑

原设计注释中"随机 → 无局部浓淡"是认知错误：随机噪声恰恰**保证**了局部浓淡存在。

## 背景层结构（HetuShell）

Tauri 透明窗口下 `backdrop-filter` 无法采样桌面像素，需自造"被模糊的内容"：

| z-index | 层 | 作用 |
|---------|-----|------|
| #app 自身 | 半透明底色 | `--bg-rgba` |
| -3 | 玻璃内容层 | 提供可被模糊的"景物"（原 canvas 噪声 → 改为文本点） |
| -2 | 玻璃面层 `#glass-veil` | `backdrop-filter: blur(Npx)` 真模糊 + 主题色奶膜 |
| -1 | 磨砂颗粒层 `#app::before` | 表面质感（canvas 噪点，无模糊） |

## 修复方案：确定性文本点替代 canvas 随机噪声

用确定性的、稀疏的文本元素替代随机噪声，从根源消除统计聚集。

### index.html

```html
<div id="glass-content" aria-hidden="true">
  <span class="gc-dot"></span><span class="gc-dot"></span>
  <span class="gc-dot"></span><span class="gc-dot"></span>
</div>
```

### CSS

```css
:root[data-glass="1"] #glass-content {
  display: block;
  position: absolute;
  inset: 0;
  z-index: -3;
  pointer-events: none;
  border-radius: inherit;
  overflow: hidden;
}

:root[data-glass="1"] #glass-content .gc-dot {
  position: absolute;
  font-size: 4px;
  line-height: 1;
  color: color-mix(in srgb, var(--term-fg) 12%, var(--term-bg));
  filter: blur(4px);
}

/* 4 个点分布在四象限中心 */
:root[data-glass="1"] #glass-content .gc-dot:nth-child(1) { top: 25%; left: 25%; }
:root[data-glass="1"] #glass-content .gc-dot:nth-child(2) { top: 25%; left: 75%; }
:root[data-glass="1"] #glass-content .gc-dot:nth-child(3) { top: 75%; left: 25%; }
:root[data-glass="1"] #glass-content .gc-dot:nth-child(4) { top: 75%; left: 75%; }

#glass-content { display: none; }
```

### themes.ts

移除 canvas 噪声生成（`frostNoiseUrl` + `GLASS_NOISE_ALPHA`），
文本点颜色由 CSS `color-mix()` 自动跟随主题，无需 JS 干预。

## 为什么有效

| 水渍根因 | 文本点方案 | 原因 |
|---------|-----------|------|
| 随机噪声统计聚集 | 消除 | 位置确定、无随机涨落 |
| 纹理平铺接缝 | 消除 | 不平铺、无周期边界 |
| 低 alpha 稀疏亮点扩散 | 消除 | 颜色可控、非随机亮暗 |

确定性文本点经模糊后形成均匀钟形光晕，无统计波动。

## 注意事项

- 4 个点在大屏上纹理密度不足，已扩展到 16 个点（4×4 网格，间距 25%）
- 16 点 CSS 定位用 `nth-child(1..16)` + `top/left` 百分比，覆盖 12.5%→87.5% 范围
- 文本点不跟随分屏布局变化——用百分比定位覆盖全屏即可
- 磨砂层（`#app::before`）仍可保留 canvas 噪点（它不做 blur，无水渍问题）
- `#glass-veil` 的 `backdrop-filter` 继续负责真模糊，层级关系不变
- 备选方案：用 CSS `radial-gradient` 替代文本点（纯 CSS、无 DOM 节点）
- 实测 4 点不足后改为 16 点效果更均匀
