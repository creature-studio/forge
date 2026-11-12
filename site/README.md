# Forge 宣传页

纯静态站点：HTML + CSS + 少量原生 JavaScript，无构建步骤、无外部字体和框架依赖。

## 本地预览

```bash
python3 -m http.server 4173 --directory site
# 打开 http://localhost:4173/
```

## GitHub Pages 自动发布

站点由 GitHub Actions 自动发布，Pages 来源设置为 **GitHub Actions**：

- 工作流：`.github/workflows/pages.yml`
- 触发条件：推送 `master` 且改动 `site/` 或工作流文件；也可以在 Actions 页面手动运行。
- 构建产物：`site/` 目录，由 GitHub 官方 Pages actions 上传并部署。
- 线上地址：`https://creature-studio.github.io/forge/`
- 本地修改 `site/` 后提交并推送 `master` 即可，不需要维护额外的发布分支。
- 如果绑定自定义域名，请全局替换以下文件中的站点地址：
  - `index.html`：`<link rel="canonical">`、`og:url`、`og:image`、`twitter:image`、JSON-LD 中的 URL
  - `robots.txt`：`Sitemap`
  - `sitemap.xml`：`<loc>` 与 `<image:loc>`

## 页面结构

- `index.html`：语义化单页，含 SoftwareApplication、WebSite、FAQPage 结构化数据
- `styles.css`：暗色终端视觉、响应式布局、入场与滚动动效
- `app.js`：移动端导航、滚动显现、安装命令复制（渐进增强）
- `assets/`：真实 TUI 截图、响应式 WebP、1200×630 社交分享图
- `robots.txt`、`sitemap.xml`、`site.webmanifest`、`favicon.svg`

## SEO 要点

- 单一 `<h1>`，按 section 使用 `<h2>` / `<h3>`，内容不依赖 JavaScript 渲染
- 独立 title、description、canonical、Open Graph 与 Twitter Card
- 图片均有描述性 alt、固定宽高与响应式 `srcset`
- FAQ 内容直接在 HTML 中，并同步到 FAQPage JSON-LD
- 预加载首屏 WebP，非首屏截图懒加载
- 支持 `prefers-reduced-motion`，键盘焦点可见，提供 skip link

## 截图

页面中的两张截图来自实际运行的 Forge（180×42 终端），演示仓库为 `aurora`，不是设计稿。
