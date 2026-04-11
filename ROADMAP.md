# epub-rs Development Roadmap

## Phase 1: 基础解析 (Core Parsing) - **[Completed]**
*   实现 `archive` 包装，能够从文件中解压。
*   解析 `META-INF/container.xml` 定位 `.opf`。
*   解析 `.opf` 文件，提取 `Metadata` (元数据), `Manifest` (资源列表), `Spine` (阅读顺序)。

## Phase 2: 内容提取与修改 (Content Processing) - **[Completed]**
*   引入并封装 `lol_html` 处理器。
*   实现从指定 href 读取对应的 HTML 字节流。
*   实现简单的 API，例如：`epub.extract_text("chapter1.xhtml")`。
*   支持流式重写（链接替换、CSS清理等）。

## Phase 3: 生成基础结构 (Core Generation) - **[Completed]**
*   实现 `EpubBuilder`。
*   正确写入 `mimetype` (Store 模式，排在首位且不压缩)。
*   自动生成基础的 `.opf` 描述文件和 ZIP 归档逻辑。

## Phase 4: 高级功能与生态 (Advanced & Tooling) - **[Completed]**
*   **EPUB 3 导航支持**：自动生成 `nav.xhtml` (EPUB 3) 和兼容的 `.ncx` (EPUB 2)。
*   **CFI 支持**：参考 `epub.js`，实现对 EPUB CFI (Canonical Fragment Identifier) 的生成与解析。

---
**🚀 所有基础架构与核心路线图已全部开发完成！**
