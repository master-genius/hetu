# Graph Report - hetu  (2026-07-05)

## Corpus Check
- 34 files · ~75,658 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 912 nodes · 1475 edges · 79 communities (77 shown, 2 thin omitted)
- Extraction: 97% EXTRACTED · 3% INFERRED · 0% AMBIGUOUS · INFERRED: 51 edges (avg confidence: 0.8)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `c1c38f38`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- [[_COMMUNITY_Community 0|Community 0]]
- [[_COMMUNITY_Community 1|Community 1]]
- [[_COMMUNITY_Community 2|Community 2]]
- [[_COMMUNITY_Community 3|Community 3]]
- [[_COMMUNITY_Community 4|Community 4]]
- [[_COMMUNITY_Community 5|Community 5]]
- [[_COMMUNITY_Community 6|Community 6]]
- [[_COMMUNITY_Community 7|Community 7]]
- [[_COMMUNITY_Community 8|Community 8]]
- [[_COMMUNITY_Community 9|Community 9]]
- [[_COMMUNITY_Community 10|Community 10]]
- [[_COMMUNITY_Community 11|Community 11]]
- [[_COMMUNITY_Community 12|Community 12]]
- [[_COMMUNITY_Community 13|Community 13]]
- [[_COMMUNITY_Community 14|Community 14]]
- [[_COMMUNITY_Community 15|Community 15]]
- [[_COMMUNITY_Community 16|Community 16]]
- [[_COMMUNITY_Community 17|Community 17]]
- [[_COMMUNITY_Community 18|Community 18]]
- [[_COMMUNITY_Community 19|Community 19]]
- [[_COMMUNITY_Community 20|Community 20]]
- [[_COMMUNITY_Community 21|Community 21]]
- [[_COMMUNITY_Community 22|Community 22]]
- [[_COMMUNITY_Community 23|Community 23]]
- [[_COMMUNITY_Community 24|Community 24]]
- [[_COMMUNITY_Community 25|Community 25]]
- [[_COMMUNITY_Community 26|Community 26]]
- [[_COMMUNITY_Community 27|Community 27]]
- [[_COMMUNITY_Community 28|Community 28]]
- [[_COMMUNITY_Community 29|Community 29]]
- [[_COMMUNITY_Community 30|Community 30]]
- [[_COMMUNITY_Community 31|Community 31]]
- [[_COMMUNITY_Community 32|Community 32]]
- [[_COMMUNITY_Community 33|Community 33]]
- [[_COMMUNITY_Community 34|Community 34]]
- [[_COMMUNITY_Community 35|Community 35]]
- [[_COMMUNITY_Community 36|Community 36]]
- [[_COMMUNITY_Community 37|Community 37]]
- [[_COMMUNITY_Community 38|Community 38]]
- [[_COMMUNITY_Community 39|Community 39]]
- [[_COMMUNITY_Community 40|Community 40]]
- [[_COMMUNITY_Community 41|Community 41]]
- [[_COMMUNITY_Community 42|Community 42]]
- [[_COMMUNITY_Community 43|Community 43]]
- [[_COMMUNITY_Community 44|Community 44]]
- [[_COMMUNITY_Community 45|Community 45]]
- [[_COMMUNITY_Community 46|Community 46]]
- [[_COMMUNITY_Community 47|Community 47]]
- [[_COMMUNITY_Community 48|Community 48]]
- [[_COMMUNITY_Community 49|Community 49]]
- [[_COMMUNITY_Community 50|Community 50]]
- [[_COMMUNITY_Community 51|Community 51]]
- [[_COMMUNITY_Community 52|Community 52]]
- [[_COMMUNITY_Community 53|Community 53]]
- [[_COMMUNITY_Community 54|Community 54]]
- [[_COMMUNITY_Community 55|Community 55]]
- [[_COMMUNITY_Community 56|Community 56]]
- [[_COMMUNITY_Community 57|Community 57]]
- [[_COMMUNITY_Community 58|Community 58]]
- [[_COMMUNITY_Community 59|Community 59]]
- [[_COMMUNITY_Community 60|Community 60]]
- [[_COMMUNITY_Community 62|Community 62]]
- [[_COMMUNITY_Community 63|Community 63]]
- [[_COMMUNITY_Community 64|Community 64]]
- [[_COMMUNITY_Community 65|Community 65]]
- [[_COMMUNITY_Community 67|Community 67]]
- [[_COMMUNITY_Community 69|Community 69]]
- [[_COMMUNITY_Community 72|Community 72]]
- [[_COMMUNITY_Community 74|Community 74]]
- [[_COMMUNITY_Community 75|Community 75]]
- [[_COMMUNITY_Community 77|Community 77]]
- [[_COMMUNITY_Community 78|Community 78]]
- [[_COMMUNITY_Community 81|Community 81]]
- [[_COMMUNITY_Community 88|Community 88]]
- [[_COMMUNITY_Community 89|Community 89]]

## God Nodes (most connected - your core abstractions)
1. `allow` - 74 edges
2. `deny` - 74 edges
3. `Pane` - 33 edges
4. `permissions` - 31 edges
5. `TabManager` - 20 edges
6. `permissions` - 17 edges
7. `compilerOptions` - 15 edges
8. `permissions` - 13 edges
9. `getSettings()` - 13 edges
10. `permissions` - 11 edges

## Surprising Connections (you probably didn't know these)
- `ssh_connect()` --calls--> `establish()`  [INFERRED]
  src-tauri/src/lib.rs → /root/source/superssh/src-tauri/src/ssh/conn.rs
- `profiles_list()` --calls--> `import()`  [INFERRED]
  src-tauri/src/lib.rs → /root/source/superssh/src-tauri/src/sshcfg.rs
- `default_download_dir()` --calls--> `home_dir()`  [INFERRED]
  src-tauri/src/lib.rs → /root/source/superssh/src-tauri/src/local.rs
- `local_home()` --calls--> `home_dir()`  [INFERRED]
  src-tauri/src/lib.rs → /root/source/superssh/src-tauri/src/local.rs
- `main()` --calls--> `run()`  [INFERRED]
  /root/source/superssh/src-tauri/src/main.rs → src-tauri/src/lib.rs

## Communities (79 total, 2 thin omitted)

### Community 0 - "Community 0"
Cohesion: 0.13
Nodes (22): showConnectDialog(), showSettingsDialog(), events, bootstrap(), onSettingsChange(), initTransfers(), confirmDialog(), confirmOverwriteDialog() (+14 more)

### Community 1 - "Community 1"
Cohesion: 0.15
Nodes (13): commands, description, identifier, commands, description, identifier, allow, commands (+5 more)

### Community 2 - "Community 2"
Cohesion: 0.07
Nodes (29): commands, description, identifier, commands, description, identifier, commands, description (+21 more)

### Community 3 - "Community 3"
Cohesion: 0.06
Nodes (33): commands, description, identifier, commands, description, identifier, commands, description (+25 more)

### Community 4 - "Community 4"
Cohesion: 0.07
Nodes (34): b64decode(), profile_delete(), profile_save(), run(), session_get(), session_set(), settings_set(), main() (+26 more)

### Community 5 - "Community 5"
Cohesion: 0.04
Nodes (44): author, dependencies, @tauri-apps/api, @tauri-apps/plugin-clipboard-manager, @tauri-apps/plugin-dialog, @xterm/addon-fit, @xterm/addon-web-links, @xterm/addon-webgl (+36 more)

### Community 6 - "Community 6"
Cohesion: 0.1
Nodes (19): app, macOSPrivateApi, security, windows, build, beforeBuildCommand, beforeDevCommand, devUrl (+11 more)

### Community 7 - "Community 7"
Cohesion: 0.04
Nodes (49): commands, description, identifier, commands, description, identifier, commands, description (+41 more)

### Community 8 - "Community 8"
Cohesion: 0.12
Nodes (15): compilerOptions, esModuleInterop, isolatedModules, lib, module, moduleResolution, noEmit, noFallthroughCasesInSwitch (+7 more)

### Community 9 - "Community 9"
Cohesion: 0.15
Nodes (13): definitions, Number, PermissionEntry, Target, Value, anyOf, description, anyOf (+5 more)

### Community 10 - "Community 10"
Cohesion: 0.07
Nodes (36): Error, AppState, get_conn(), pane_input(), pane_open(), read_key_file(), register_transfer(), remote_cwd() (+28 more)

### Community 11 - "Community 11"
Cohesion: 0.11
Nodes (23): AppImage 无法运行 / 提示二进制不完整, code:bash (git tag v0.1.0), code:bash (# 1. Rust 工具链（一次性）), code:bash (# A. 免 FUSE 直接解压运行（最省事）), code:bash (WEBKIT_DISABLE_DMABUF_RENDERER=1 ./HetuShell        # 常见花屏修复), code:bash (PROMPT_COMMAND='printf "\e]7;file://%s%s\e\\" "$HOSTNAME" "$), code:block6 (src/            前端（TypeScript + xterm.js，无重框架）), HetuShell 河图终端 (+15 more)

### Community 12 - "Community 12"
Cohesion: 0.15
Nodes (13): definitions, Number, PermissionEntry, Target, Value, anyOf, description, anyOf (+5 more)

### Community 14 - "Community 14"
Cohesion: 0.07
Nodes (6): Layout, LayoutNode, SplitDir, Pane, wordAt(), wordRangeAt()

### Community 15 - "Community 15"
Cohesion: 0.2
Nodes (10): description, properties, required, type, Capability, type, identifier, remote (+2 more)

### Community 16 - "Community 16"
Cohesion: 0.2
Nodes (10): $ref, description, items, type, uniqueItems, description, items, type (+2 more)

### Community 17 - "Community 17"
Cohesion: 0.2
Nodes (10): type, webviews, windows, items, description, items, type, description (+2 more)

### Community 18 - "Community 18"
Cohesion: 0.2
Nodes (10): $ref, description, items, type, uniqueItems, description, items, type (+2 more)

### Community 19 - "Community 19"
Cohesion: 0.2
Nodes (10): type, webviews, windows, items, description, items, type, description (+2 more)

### Community 20 - "Community 20"
Cohesion: 0.23
Nodes (15): activeTheme(), applySettings(), fontStack(), getSettings(), listeners, loadSettings(), updateSettings(), allThemes() (+7 more)

### Community 21 - "Community 21"
Cohesion: 0.15
Nodes (13): commands, description, identifier, deny, commands, description, identifier, commands (+5 more)

### Community 22 - "Community 22"
Cohesion: 0.2
Nodes (10): description, properties, required, type, Capability, type, identifier, remote (+2 more)

### Community 23 - "Community 23"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-fetch-data-store-identifiers

### Community 24 - "Community 24"
Cohesion: 0.25
Nodes (8): description, properties, required, type, CapabilityRemote, urls, description, type

### Community 25 - "Community 25"
Cohesion: 0.25
Nodes (8): description, properties, required, type, CapabilityRemote, urls, description, type

### Community 26 - "Community 26"
Cohesion: 0.25
Nodes (6): default, description, identifier, local, permissions, windows

### Community 27 - "Community 27"
Cohesion: 0.48
Nodes (5): description, identifier, permissions, $schema, windows

### Community 28 - "Community 28"
Cohesion: 0.1
Nodes (21): commands, description, identifier, permissions, commands, description, identifier, commands (+13 more)

### Community 29 - "Community 29"
Cohesion: 0.15
Nodes (11): CATEGORY, Explorer, ExplorerBackend, fileSvg(), folderSvg(), FsEntry, ICON_MAP, iconFor() (+3 more)

### Community 30 - "Community 30"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-prepend

### Community 31 - "Community 31"
Cohesion: 0.53
Nodes (4): anyOf, description, $schema, title

### Community 32 - "Community 32"
Cohesion: 0.53
Nodes (4): anyOf, description, $schema, title

### Community 33 - "Community 33"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-identifier

### Community 34 - "Community 34"
Cohesion: 0.22
Nodes (15): default_download_dir(), local_home(), local_list(), profiles_list(), default_shell(), home_dir(), list_dir(), LocalEntry (+7 more)

### Community 35 - "Community 35"
Cohesion: 0.22
Nodes (15): api, b64encode(), WordRange, Tab, ConnParams, ConnStateEvent, FileMeta, LocalEntry (+7 more)

### Community 36 - "Community 36"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-name

### Community 37 - "Community 37"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-set-dock-visibility

### Community 38 - "Community 38"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-tauri-version

### Community 39 - "Community 39"
Cohesion: 0.2
Nodes (16): beginTransfer(), completeTransfer(), ensurePanel(), failTransfer(), hidePanel(), ICON, refreshVisibility(), removeRow() (+8 more)

### Community 40 - "Community 40"
Cohesion: 0.12
Nodes (26): clipboard-manager, default_permission, global_scope_schema, permission_sets, core, core:app, default_permission, global_scope_schema (+18 more)

### Community 41 - "Community 41"
Cohesion: 0.07
Nodes (29): commands, description, identifier, commands, description, identifier, commands, description (+21 more)

### Community 42 - "Community 42"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-tauri-version

### Community 43 - "Community 43"
Cohesion: 0.26
Nodes (10): CJK_FONTS, MONO_FONTS, THEME_COLOR_KEYS, Action, ACTIONS, comboToLabel(), DEFAULT_KEYBINDINGS, eventToCombo() (+2 more)

### Community 44 - "Community 44"
Cohesion: 0.53
Nodes (4): b64(), open(), PaneCmd, PaneCtl

### Community 45 - "Community 45"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-bundle-type

### Community 46 - "Community 46"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-app-hide

### Community 47 - "Community 47"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-set-app-theme

### Community 48 - "Community 48"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-default-window-icon

### Community 49 - "Community 49"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-supports-multiple-windows

### Community 50 - "Community 50"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-version

### Community 51 - "Community 51"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-default-window-icon

### Community 52 - "Community 52"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-from-bytes

### Community 53 - "Community 53"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-remove-data-store

### Community 54 - "Community 54"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-identifier

### Community 55 - "Community 55"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-bundle-type

### Community 56 - "Community 56"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-remove-data-store

### Community 57 - "Community 57"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-set-app-theme

### Community 58 - "Community 58"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-from-path

### Community 59 - "Community 59"
Cohesion: 0.67
Nodes (3): Identifier, description, oneOf

### Community 60 - "Community 60"
Cohesion: 0.67
Nodes (3): Identifier, description, oneOf

### Community 62 - "Community 62"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-is-enabled

### Community 63 - "Community 63"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-remove-at

### Community 64 - "Community 64"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-remove

### Community 65 - "Community 65"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-create-default

### Community 67 - "Community 67"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-is-checked

### Community 69 - "Community 69"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-popup

### Community 72 - "Community 72"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-set-as-help-menu-for-nsapp

### Community 74 - "Community 74"
Cohesion: 0.5
Nodes (4): default, description, type, local

### Community 75 - "Community 75"
Cohesion: 0.5
Nodes (4): default, description, type, description

### Community 77 - "Community 77"
Cohesion: 0.5
Nodes (4): default, description, type, local

### Community 78 - "Community 78"
Cohesion: 0.5
Nodes (4): default, description, type, description

### Community 88 - "Community 88"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-register-listener

### Community 89 - "Community 89"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-from-bytes

## Knowledge Gaps
- **334 isolated node(s):** `target`, `module`, `moduleResolution`, `lib`, `strict` (+329 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **2 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `allow` connect `Community 1` to `Community 2`, `Community 3`, `Community 7`, `Community 21`, `Community 23`, `Community 28`, `Community 30`, `Community 33`, `Community 36`, `Community 37`, `Community 38`, `Community 41`, `Community 42`, `Community 45`, `Community 46`, `Community 47`, `Community 48`, `Community 49`, `Community 50`, `Community 51`, `Community 52`, `Community 53`, `Community 54`, `Community 55`, `Community 56`, `Community 57`, `Community 58`, `Community 62`, `Community 63`, `Community 64`, `Community 65`, `Community 67`, `Community 69`, `Community 72`, `Community 88`, `Community 89`?**
  _High betweenness centrality (0.046) - this node is a cross-community bridge._
- **Why does `deny` connect `Community 21` to `Community 1`, `Community 2`, `Community 3`, `Community 7`, `Community 23`, `Community 28`, `Community 30`, `Community 33`, `Community 36`, `Community 37`, `Community 38`, `Community 41`, `Community 42`, `Community 45`, `Community 46`, `Community 47`, `Community 48`, `Community 49`, `Community 50`, `Community 51`, `Community 52`, `Community 53`, `Community 54`, `Community 55`, `Community 56`, `Community 57`, `Community 58`, `Community 62`, `Community 63`, `Community 64`, `Community 65`, `Community 67`, `Community 69`, `Community 72`, `Community 88`, `Community 89`?**
  _High betweenness centrality (0.046) - this node is a cross-community bridge._
- **Why does `Pane` connect `Community 14` to `Community 0`, `Community 35`, `Community 20`, `Community 4`?**
  _High betweenness centrality (0.040) - this node is a cross-community bridge._
- **What connects `target`, `module`, `moduleResolution` to the rest of the system?**
  _334 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Community 0` be split into smaller, more focused modules?**
  _Cohesion score 0.13 - nodes in this community are weakly interconnected._
- **Should `Community 2` be split into smaller, more focused modules?**
  _Cohesion score 0.07 - nodes in this community are weakly interconnected._
- **Should `Community 3` be split into smaller, more focused modules?**
  _Cohesion score 0.06 - nodes in this community are weakly interconnected._