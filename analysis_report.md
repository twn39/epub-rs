# epub-rs 综合分析报告

> 生成时间：2026-05-05  
> 基准版本：当前 main 分支  
> 对比参考：`go-toolkit v0.13.4`、`epub.js`、`ebooklib`

---

## 一、整体架构评估

### 1.1 核心设计优势

| 维度 | 评价 |
|------|------|
| 模块化 | ✅ 优秀：`parser/opf`、`navigation`、`positions` 各自独立，职责清晰 |
| 跨平台 | ✅ 优秀：统一核心逻辑，通过条件编译分别支持 WASM / native / C FFI |
| Provider 抽象 | ✅ 优秀：`EpubProvider` trait 屏蔽 ZIP/Dir 差异，测试和扩展都方便 |
| 错误处理 | ✅ 良好：`thiserror` 定义 `EpubError`，边界一致 |
| FFI 安全 | ✅ 良好：JSON 作为复杂返回值，`panic = "abort"` 防止 UB 越界 |
| 元数据模型 | ⚠️ 中等：核心字段完备，但部分高级字段尚缺 |
| 测试覆盖 | ⚠️ 中等：主路径有覆盖，但缺少 fuzzing 和 malformed-EPUB 边界测试 |
| Benchmarks | ❌ 不足：`benches/` 未覆盖 `positions_by_reading_order` 等关键热路径 |

---

## 二、解析器（Parser）对比

### 2.1 OPF 元数据解析

**go-toolkit 能做，epub-rs 尚未支持：**

| 功能 | go-toolkit | epub-rs | 优先级 |
|------|-----------|---------|--------|
| 本地化标题 (`LocalizedString`, `xml:lang`) | ✅ | ❌ 仅取主 `dc:title` | HIGH |
| 副标题 (`title-type="subtitle"`) | ✅ | ❌ | MEDIUM |
| 排序标题 (`file-as`, `title_sort`) | ✅ | ⚠️ 仅存 `sort_as` 字段，未从 OPF meta refinement 解析 | MEDIUM |
| 多标识符 (`AltIdentifier`) | ✅ | ❌ 仅取第一个 `dc:identifier` | LOW |
| 可访问性元数据 (`A11y`: WCAG 等级、certifiedBy) | ✅ 完整 | ❌ 未实现 | MEDIUM |
| TDM 数字版权元数据 | ✅ | ❌ | LOW |
| 媒体覆盖时长 (`media:duration`) | ✅ | ❌ | LOW |
| 多语言字段 (`languages[]` 完整列表) | ✅ | ⚠️ `languages` Vec 存在，OPF 多 `dc:language` 是否全部捕获待确认 |

**epub-rs 已实现：**
- `dc:title/creator/language/identifier/publisher/description/date/rights/subject`
- EPUB 3 `meta refines` 角色与文件排序
- `BelongsTo`（系列、合集）、`ReadingProgression`（RTL 自动推断）
- `LayoutType`（反排/预分页）、`SpineItem.linear` 正确过滤

### 2.2 加密/混淆支持

| 功能 | epub-rs | 说明 |
|------|---------|------|
| IDPF 字体混淆/解混淆 | ✅ | `DeobfuscatingReader` 正确实现 SHA-1 密钥 XOR |
| Adobe 字体混淆/解混淆 | ✅ | 同上，使用 MD5 密钥 |
| `encryption.xml` 内容加密（DRM） | ❌ | 仅跳过加密资源，不支持 AES 解密 |
| FFI 层加密解密 | ✅ | `epub_decrypt_font` 已完整导出 |

---

## 三、导航（Navigation）对比

### 3.1 功能矩阵

| 功能 | epub-rs | go-toolkit | epub.js |
|------|---------|-----------|---------|
| EPUB 3 nav.xhtml 单次 DOM 遍历 | ✅ 优秀 | ✅ | ✅ |
| EPUB 2 NCX 单次流式解析 | ✅ | ✅ | ✅ |
| 嵌套 TOC | ✅ 递归正确 | ✅ | ✅ |
| epub:type 多 token 支持 | ✅ | ✅ | 部分 |
| 无 epub:type 的 fallback（`nav#toc` → 首个 nav） | ✅ | ✅ | ✅ |
| `landmarks` | ✅ | ✅ | ✅ |
| `page-list` | ✅ | ✅ | ✅ |
| NCX `navPoint` 标题无文本时的降级 | ✅ | ✅ | ✅ |
| SMIL/Audio 媒体覆盖 | ❌ | ✅ | ⚠️ 基础 |

**gap：SMIL Media Overlay**  
go-toolkit 实现了完整的 `MediaOverlayService`（`media_overlay_service.go`），可解析 SMIL 文件并生成 `GuidedNavigationDocument`，支持 TTS（文字转语音）同步功能。epub-rs 对此无任何支持。

---

## 四、定位系统（Positions）

### 4.1 实现正确性

| 指标 | 评价 |
|------|------|
| Adobe RMSDK 1024 字节/位置 默认值 | ✅ 完全对应 |
| `non-linear` spine 项目排除 | ✅ 正确 |
| Fixed-layout 始终 1 个位置 | ✅ 正确 |
| `global_position` 单调递增 | ✅ 与 go-toolkit 公式一致 |
| `chapter_progression` / `total_progression` | ✅ 公式与 go-toolkit 对齐 |
| CFI 生成（位置 0 → `/4`，位置 N → `/4/N*2`） | ✅ 符合 CFI 规范 |
| `generate_locations()` 快捷方法 | ✅ |

**可优化项：**
- `positions_by_reading_order` 内部调用了 `get_toc()` 来构建 title map，这会导致额外的 I/O（读导航文件一次）。若调用方已经缓存了 TOC，可提供一个接受外部 `title_map` 的重载版本以避免重复 I/O。
- 目前 `benches/` 未覆盖此函数，大 EPUB 下的性能回归无法自动检测。

---

## 五、CFI 实现

### 5.1 对比 epub.js

epub-rs 的 CFI 实现相当完善：

| 功能 | epub-rs | epub.js |
|------|---------|---------|
| Point CFI 解析 | ✅ | ✅ |
| Range CFI 解析 | ✅ | ✅ |
| `id_shortcut`（O(1) getElementById） | ✅ | ✅ |
| XPath 回退 | ✅ | ✅ |
| `is_text_node` + `character_offset` | ✅ | ✅ |
| CFI 比较（`Ord` trait） | ✅ | ✅ |
| CFI Range 生成（公共路径提取） | ✅ | ✅ |
| CFI `assert` 侧路径（`[id]`）验证 | ⚠️ 存储但未验证 | ✅ 可选验证 |
| Temporal CFI（音视频时间偏移） | ❌ | ✅ |
| Spatial CFI（图像坐标） | ❌ | ⚠️ 基础 |

**gap：Temporal/Spatial CFI**  
epub-rs 未实现用于音视频 EPUB 的 Temporal CFI（`~[time]`）和用于图像书的 Spatial CFI（`@x:y`）。这些在固定布局漫画书或有声书场景下是必须的。

---

## 六、内容处理器（Processor）

### 6.1 功能状态

| 功能 | 状态 | 备注 |
|------|------|------|
| HTML 文本提取 (`extract_text`) | ✅ | 基于 lol_html |
| 流式文本提取 (`extract_text_stream`) | ✅ | |
| 资源链接重写 (`rewrite_resources`) | ✅ | 支持 `<img>`, `<link>`, `<a>` 等 |
| `<head>` 内容注入 | ✅ | |
| CFI DOM 注入 (`inject_cfi_dom`) | ✅ | `data-cfi` 属性注入 |
| 文本搜索 (`search_chapter`) | ✅ | 正则表达式匹配，返回 `SearchResult[]` |
| 语义内容提取 (`extract_semantic_content`) | ✅ | 段落/标题的 TTS 结构化输出 |
| HTML → EPUB 的外部文件导入 | ✅ | `add_chapter_from_html_file` 含链接扫描 |
| MathML 渲染/处理 | ❌ | |
| SVG 内容处理 | ❌ | |
| Ruby 注音 (`<ruby>`) 提取 | ❌ | 对中日韩语言很重要 |

**架构建议：**  
`processor.rs` 目前是单文件（约 800+ 行），随功能增加维护难度上升。建议按如下方式拆分为目录模块：

```
src/processor/
├── mod.rs       # re-exports
├── cfi.rs       # inject_cfi_dom, search_chapter
├── html.rs      # extract_text, rewrite_resources, inject_head_content
└── semantic.rs  # extract_semantic_content
```

---

## 七、生成器（Generator）

### 7.1 功能矩阵

| 功能 | 状态 |
|------|------|
| EPUB 2/3 基础结构生成 | ✅ |
| `mimetype` 首个未压缩条目 | ✅ 规范遵从 |
| OPF 元数据写入 | ✅ |
| nav.xhtml（EPUB 3） | ✅ 含 TOC/landmarks/page-list |
| NCX（EPUB 2 + EPUB 3 兼容回退） | ✅ |
| 封面图片 (`cover-image` property) | ✅ |
| 流式资源添加 (`add_resource_stream`) | ✅ |
| 固定布局 spine 属性 | ✅ `add_chapter_with_layout` |
| Modern 主题 CSS 注入 | ✅ |
| HTML 文件导入（含资源扫描） | ✅ native-only |
| 验证 (`validate()`) | ✅ |
| 加密写入（DRM） | ❌ |
| 媒体覆盖（SMIL 生成） | ❌ |
| 元数据全字段往返 (subtitle/sort_as/modified等) | ⚠️ 模型有字段，OPF 写入可能不完整 |

**已知 bug 风险：**  
`generate()` 中主题 CSS 注入逻辑通过计算 `href` 中 `/` 数量推断目录深度（第 655 行）。这是一种启发式方法，当 `href` 路径层次不规则时（如直接 `chapter.xhtml` 无子目录），会产生错误的相对路径 `../../styles/...`。建议改用规范的相对路径计算函数。

---

## 八、C FFI 层

### 8.1 当前函数清单

**解析器 API（完整）：**
- `epub_open` / `epub_open_file` / `epub_free`
- `epub_parse`
- `epub_get_navigation` / `epub_get_toc` / `epub_get_page_list`
- `epub_positions_by_reading_order` / `epub_generate_locations`
- `epub_get_cover_image` / `epub_get_resource` / `epub_get_resource_by_id`
- `epub_get_chapter_with_cfi` / `epub_search_chapter` / `epub_get_semantic_content`

**CFI 工具（无状态）：**
- `epub_resolve_cfi` / `epub_compare_cfi` / `epub_generate_cfi_range`

**加密：**
- `epub_decrypt_font`

**生成器 API（完整）：**
- `epub_generator_new` / `epub_generator_free`
- `epub_generator_set_title` / `set_language` / `set_identifier` / `add_author`
- `epub_generator_add_chapter` / `add_chapter_with_nav` / `add_resource` / `set_cover`
- `epub_generator_add_landmark` / `add_page`
- `epub_generator_build`

### 8.2 FFI 待完善项

| 缺口 | 描述 | 优先级 |
|------|------|--------|
| `epub_generator_set_toc` | 通过 JSON 字符串设置完整嵌套 TOC（WASM 层已有，FFI 层缺失） | HIGH |
| `epub_generator_validate` | 构建前验证接口（错误通过 `epub_last_error()` 获取） | MEDIUM |
| `epub_generator_set_metadata` | 通过 JSON 一次性设置全部元数据（与 WASM 对称） | MEDIUM |
| `epub_get_positions_flat` | 别名：`epub_generate_locations` 已实现此语义，函数名不一致 | LOW（重命名） |
| Swift Package/Python ctypes 示例 | 目前 `include/epub_rs.h` 存在，但无调用示例 | MEDIUM |

---

## 九、WASM 层

### 9.1 功能完整性

WASM 层功能最完整，与 Rust native 核心的对称性最高：

- `EpubParser`：parse、get_toc、get_page_list、get_navigation、get_cover_image、generate_locations、positions_by_reading_order、get_chapter_with_rewritten_assets ✅
- `EpubGenerator`：完整元数据、章节、TOC、封面、landmark、page ✅
- 独立工具函数：resolve_cfi、compare_cfi、generate_cfi_range、inject_cfi_markers、search_text_in_chapter、extract_semantic_content、inject_script_and_style、decrypt_font ✅

**WASM 待完善：**
1. `positions_by_reading_order` 目前创建内部 `ArchiveEntryLength` strategy，无法接受自定义字节/位置比率（策略不可插拔）。
2. 缺少 `get_renditions()` 导出——多 rendition EPUB（含 FXL + Reflowable 双版本）无法从 WASM 访问。

---

## 十、测试覆盖

### 10.1 现有测试

| 测试文件 | 内容 |
|----------|------|
| `tests/parser_tests.rs` | 真实 EPUB 解析、DirProvider、多 rendition、错误处理、相对路径解析 |
| `src/parser/navigation.rs` (inline) | NCX 和 nav.xhtml 的所有路径（7 个测试） |
| `src/parser/positions.rs` (inline) | — （无 inline 测试） |
| `src/provider.rs` (inline) | ZIP 查找（精确/大小写/扩展别名）、path traversal 拒绝 |
| `src/model.rs` (inline) | RTL 推断、系列字段、serde 往返 |
| `src/ffi.rs` (inline) | open/parse/toc/null 安全/CFI 解析 |
| `src/wasm.rs` (inline) | 完整 EPUB 生成+解析往返、crypto、asset 重写、验证错误 |

### 10.2 测试空白

| 空白 | 影响 |
|------|------|
| OPF 解析 `opf.rs` 无 inline 测试 | 核心解析路径没有单元测试保护 |
| `positions.rs` 无 inline 测试 | 位置算法改动无测试回归 |
| `cfi.rs` 解析器无 inline 测试 | CFI 格式变更风险 |
| `crypto.rs` 解析测试 | 仅 WASM 层有覆盖，native/FFI 层缺少 |
| malformed OPF 样本（EPUB 真实世界边界情况） | `tests/` 仅有少量人工构造 |
| fuzzing（cargo-fuzz / libfuzzer） | 无 |
| Benchmarks 覆盖 | `benches/` 内容未知，但未包含关键热路径 |

---

## 十一、与 go-toolkit 的结构性差异

| 维度 | go-toolkit | epub-rs | 影响 |
|------|-----------|---------|------|
| 流式资源访问 | `Fetcher` 接口，惰性 IO | `EpubProvider` 全量读取 | epub-rs 大文件全读入内存，go-toolkit 支持惰性流 |
| 出版物服务架构 | `pub.Service` 插件体系 | 无插件体系 | epub-rs 功能扩展需修改核心代码 |
| Media Overlay | 完整 SMIL 解析服务 | 不支持 | 有声书/TTS 场景缺失 |
| 可访问性元数据 | 完整 A11y 模型 | 无 | 无障碍合规场景缺失 |
| 本地化字符串 | `LocalizedString` 多语言支持 | 仅单一字符串字段 | 多语言元数据损失 |
| Guided Navigation | SMIL GuidedNavigationDoc | 无 | 朗读同步功能缺失 |

---

## 十二、优化优先级列表

### P0 – 功能正确性修复

1. **generator CSS 路径 bug**：修复通过斜杠计数推断目录深度的启发式逻辑（`generator.rs:655`），改用正确的相对路径算法。

### P1 – 高价值功能缺口

2. **OPF 本地化标题解析**：支持 `xml:lang`、`title-type` refinements、`file-as`，使元数据与 go-toolkit 对齐。
3. **SMIL Media Overlay 解析**（基础版）：至少支持检测 spine item 是否有 SMIL 关联，提供章节-时间戳映射。
4. **FFI `epub_generator_set_toc`**：将 WASM 已有的 `set_toc(JSON)` 接口移植到 FFI 层，保持对称。
5. **`processor` 模块拆分**：将 `processor.rs` 拆分为 `processor/` 目录（`html.rs`, `cfi.rs`, `semantic.rs`）。

### P2 – 测试质量

6. **`src/parser/opf.rs` inline tests**：为状态机的各个路径添加单元测试。
7. **`src/parser/positions.rs` inline tests**：位置计算公式测试，包含边界（单章 EPUB、固定布局）。
8. **CFI 测试套件**：range CFI 生成、compare 排序、id_shortcut 提取。
9. **Benchmark 补充**：`benches/` 添加 `positions_by_reading_order` 和 `inject_cfi_dom` 基准。

### P3 – 可选增强

10. **Temporal CFI 支持**（`~[time]`）：面向音视频 EPUB 场景。
11. **Spatial CFI 支持**（`@x:y`）：面向图像/漫画书场景。
12. **可访问性元数据模型**：`A11y`、WCAG 等级、`certifiedBy` 字段解析。
13. **WASM 多 rendition 导出**：暴露 `get_renditions()` 到 WASM 层。
14. **Cargo fuzz 集成**：对 ZIP 解析、OPF 解析、CFI 解析添加 libfuzzer 目标。
15. **Swift/Python 集成示例**：在 `examples/` 下添加实际调用 C FFI 的代码示例。

---

## 十三、总结

`epub-rs` 已具备工业级基础：
- 解析器状态机健壮，OPF/NCX/nav.xhtml 路径均有覆盖
- CFI 实现功能完整，与 epub.js 基本对齐
- Provider 抽象优雅，ZIP/Dir 双模式支持
- WASM/FFI 双层导出，跨语言能力完善
- 生成器 Builder API 设计良好

**核心差距**集中在：
1. 元数据丰富度（本地化、可访问性）——与 go-toolkit 差距最大
2. 媒体覆盖/SMIL——有声书场景完全缺失
3. 测试覆盖深度——opf 核心路径无单元测试
4. Benchmarks——缺少热路径回归检测

这些差距均可逐步填补，且不影响现有 API 的稳定性。
