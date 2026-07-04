# Graph Report - src-tauri  (2026-07-04)

## Corpus Check
- 17 files · ~27,382 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 592 nodes · 824 edges · 72 communities
- Extraction: 96% EXTRACTED · 4% INFERRED · 0% AMBIGUOUS · INFERRED: 33 edges (avg confidence: 0.8)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `b5d0ecf7`
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
- [[_COMMUNITY_Community 61|Community 61]]
- [[_COMMUNITY_Community 62|Community 62]]
- [[_COMMUNITY_Community 63|Community 63]]
- [[_COMMUNITY_Community 64|Community 64]]
- [[_COMMUNITY_Community 65|Community 65]]
- [[_COMMUNITY_Community 66|Community 66]]
- [[_COMMUNITY_Community 67|Community 67]]
- [[_COMMUNITY_Community 68|Community 68]]
- [[_COMMUNITY_Community 69|Community 69]]
- [[_COMMUNITY_Community 70|Community 70]]

## God Nodes (most connected - your core abstractions)
1. `allow` - 74 edges
2. `deny` - 74 edges
3. `permissions` - 31 edges
4. `permissions` - 17 edges
5. `permissions` - 13 edges
6. `permissions` - 11 edges
7. `properties` - 9 edges
8. `permissions` - 9 edges
9. `properties` - 9 edges
10. `definitions` - 8 edges

## Surprising Connections (you probably didn't know these)
- `main()` --calls--> `build`  [INFERRED]
  build.rs → tauri.conf.json
- `settings_set()` --calls--> `save()`  [INFERRED]
  src/lib.rs → src/settings.rs
- `profile_save()` --calls--> `save()`  [INFERRED]
  src/lib.rs → src/settings.rs
- `profile_delete()` --calls--> `save()`  [INFERRED]
  src/lib.rs → src/settings.rs
- `profiles_list()` --calls--> `import()`  [INFERRED]
  src/lib.rs → src/sshcfg.rs

## Communities (72 total, 0 thin omitted)

### Community 0 - "Community 0"
Cohesion: 0.04
Nodes (49): commands, description, identifier, commands, description, identifier, commands, description (+41 more)

### Community 1 - "Community 1"
Cohesion: 0.08
Nodes (33): Error, AppState, get_conn(), pane_input(), pane_open(), profile_delete(), profile_save(), remote_home() (+25 more)

### Community 2 - "Community 2"
Cohesion: 0.05
Nodes (44): commands, description, identifier, commands, description, identifier, commands, description (+36 more)

### Community 3 - "Community 3"
Cohesion: 0.06
Nodes (36): commands, description, identifier, commands, description, identifier, commands, description (+28 more)

### Community 4 - "Community 4"
Cohesion: 0.09
Nodes (16): run(), ssh_connect(), main(), config_dir(), known_hosts_path(), load(), Profile, Settings (+8 more)

### Community 5 - "Community 5"
Cohesion: 0.1
Nodes (19): main(), app, macOSPrivateApi, security, windows, build, beforeBuildCommand, beforeDevCommand (+11 more)

### Community 6 - "Community 6"
Cohesion: 0.19
Nodes (17): clipboard-manager, default_permission, global_scope_schema, permission_sets, core, default_permission, default_permission, default_permission (+9 more)

### Community 7 - "Community 7"
Cohesion: 0.23
Nodes (12): local_home(), local_list(), profiles_list(), home_dir(), list_dir(), LocalEntry, open(), size() (+4 more)

### Community 8 - "Community 8"
Cohesion: 0.15
Nodes (13): definitions, Number, PermissionEntry, Target, Value, anyOf, description, anyOf (+5 more)

### Community 9 - "Community 9"
Cohesion: 0.15
Nodes (13): definitions, Number, PermissionEntry, Target, Value, anyOf, description, anyOf (+5 more)

### Community 10 - "Community 10"
Cohesion: 0.2
Nodes (10): $ref, description, items, type, uniqueItems, description, items, type (+2 more)

### Community 11 - "Community 11"
Cohesion: 0.2
Nodes (10): properties, type, default, description, type, identifier, local, remote (+2 more)

### Community 12 - "Community 12"
Cohesion: 0.2
Nodes (10): type, webviews, windows, items, description, items, type, description (+2 more)

### Community 13 - "Community 13"
Cohesion: 0.2
Nodes (10): $ref, description, items, type, uniqueItems, description, items, type (+2 more)

### Community 14 - "Community 14"
Cohesion: 0.2
Nodes (10): properties, type, default, description, type, identifier, local, remote (+2 more)

### Community 15 - "Community 15"
Cohesion: 0.2
Nodes (10): type, webviews, windows, items, description, items, type, description (+2 more)

### Community 16 - "Community 16"
Cohesion: 0.22
Nodes (9): commands, description, identifier, allow, commands, description, identifier, allow-app-show (+1 more)

### Community 17 - "Community 17"
Cohesion: 0.22
Nodes (9): commands, description, identifier, deny, commands, description, identifier, allow-app-hide (+1 more)

### Community 18 - "Community 18"
Cohesion: 0.25
Nodes (8): core:app, global_scope_schema, permission_sets, permissions, commands, description, identifier, deny-register-listener

### Community 19 - "Community 19"
Cohesion: 0.25
Nodes (8): description, properties, required, type, CapabilityRemote, urls, description, type

### Community 20 - "Community 20"
Cohesion: 0.25
Nodes (8): description, properties, required, type, CapabilityRemote, urls, description, type

### Community 21 - "Community 21"
Cohesion: 0.29
Nodes (6): default, description, identifier, local, permissions, windows

### Community 22 - "Community 22"
Cohesion: 0.33
Nodes (5): description, identifier, permissions, $schema, windows

### Community 23 - "Community 23"
Cohesion: 0.4
Nodes (5): commands, description, identifier, permissions, allow-append

### Community 24 - "Community 24"
Cohesion: 0.4
Nodes (4): anyOf, description, $schema, title

### Community 25 - "Community 25"
Cohesion: 0.4
Nodes (4): anyOf, description, $schema, title

### Community 26 - "Community 26"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-fetch-data-store-identifiers

### Community 27 - "Community 27"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-name

### Community 28 - "Community 28"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-register-listener

### Community 29 - "Community 29"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-remove-listener

### Community 30 - "Community 30"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-tauri-version

### Community 31 - "Community 31"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-remove-listener

### Community 32 - "Community 32"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-version

### Community 33 - "Community 33"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-bundle-type

### Community 34 - "Community 34"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-default-window-icon

### Community 35 - "Community 35"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-identifier

### Community 36 - "Community 36"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-remove-data-store

### Community 37 - "Community 37"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-set-app-theme

### Community 38 - "Community 38"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-set-dock-visibility

### Community 39 - "Community 39"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-supports-multiple-windows

### Community 40 - "Community 40"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-version

### Community 41 - "Community 41"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-app-hide

### Community 42 - "Community 42"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-app-show

### Community 43 - "Community 43"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-bundle-type

### Community 44 - "Community 44"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-default-window-icon

### Community 45 - "Community 45"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-identifier

### Community 46 - "Community 46"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-name

### Community 47 - "Community 47"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-remove-data-store

### Community 48 - "Community 48"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-set-app-theme

### Community 49 - "Community 49"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-set-dock-visibility

### Community 50 - "Community 50"
Cohesion: 0.5
Nodes (4): commands, description, identifier, deny-supports-multiple-windows

### Community 51 - "Community 51"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-get

### Community 52 - "Community 52"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-is-enabled

### Community 53 - "Community 53"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-remove-at

### Community 54 - "Community 54"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-remove

### Community 55 - "Community 55"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-create-default

### Community 56 - "Community 56"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-insert

### Community 57 - "Community 57"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-is-checked

### Community 58 - "Community 58"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-items

### Community 59 - "Community 59"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-popup

### Community 60 - "Community 60"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-prepend

### Community 61 - "Community 61"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-set-accelerator

### Community 62 - "Community 62"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-set-as-app-menu

### Community 63 - "Community 63"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-set-as-help-menu-for-nsapp

### Community 64 - "Community 64"
Cohesion: 0.5
Nodes (4): commands, description, identifier, allow-set-as-window-menu

### Community 65 - "Community 65"
Cohesion: 0.5
Nodes (4): description, required, type, Capability

### Community 66 - "Community 66"
Cohesion: 0.5
Nodes (4): default, description, type, description

### Community 67 - "Community 67"
Cohesion: 0.5
Nodes (4): description, required, type, Capability

### Community 68 - "Community 68"
Cohesion: 0.5
Nodes (4): default, description, type, description

### Community 69 - "Community 69"
Cohesion: 0.67
Nodes (3): Identifier, description, oneOf

### Community 70 - "Community 70"
Cohesion: 0.67
Nodes (3): Identifier, description, oneOf

## Knowledge Gaps
- **272 isolated node(s):** `$schema`, `productName`, `version`, `identifier`, `beforeDevCommand` (+267 more)
  These have ≤1 connection - possible missing edges or undocumented components.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `allow` connect `Community 16` to `Community 0`, `Community 2`, `Community 3`, `Community 17`, `Community 18`, `Community 23`, `Community 26`, `Community 27`, `Community 28`, `Community 29`, `Community 30`, `Community 31`, `Community 32`, `Community 33`, `Community 34`, `Community 35`, `Community 36`, `Community 37`, `Community 38`, `Community 39`, `Community 40`, `Community 41`, `Community 42`, `Community 43`, `Community 44`, `Community 45`, `Community 46`, `Community 47`, `Community 48`, `Community 49`, `Community 50`, `Community 51`, `Community 52`, `Community 53`, `Community 54`, `Community 55`, `Community 56`, `Community 57`, `Community 58`, `Community 59`, `Community 60`, `Community 61`, `Community 62`, `Community 63`?**
  _High betweenness centrality (0.110) - this node is a cross-community bridge._
- **Why does `deny` connect `Community 17` to `Community 0`, `Community 2`, `Community 3`, `Community 16`, `Community 18`, `Community 23`, `Community 26`, `Community 27`, `Community 28`, `Community 29`, `Community 30`, `Community 31`, `Community 32`, `Community 33`, `Community 34`, `Community 35`, `Community 36`, `Community 37`, `Community 38`, `Community 39`, `Community 40`, `Community 41`, `Community 42`, `Community 43`, `Community 44`, `Community 45`, `Community 46`, `Community 47`, `Community 48`, `Community 49`, `Community 50`, `Community 51`, `Community 52`, `Community 53`, `Community 54`, `Community 55`, `Community 56`, `Community 57`, `Community 58`, `Community 59`, `Community 60`, `Community 61`, `Community 62`, `Community 63`?**
  _High betweenness centrality (0.110) - this node is a cross-community bridge._
- **Why does `permissions` connect `Community 18` to `Community 16`, `Community 17`, `Community 26`, `Community 27`, `Community 28`, `Community 29`, `Community 30`, `Community 31`, `Community 32`, `Community 33`, `Community 34`, `Community 35`, `Community 36`, `Community 37`, `Community 38`, `Community 39`, `Community 40`, `Community 41`, `Community 42`, `Community 43`, `Community 44`, `Community 45`, `Community 46`, `Community 47`, `Community 48`, `Community 49`, `Community 50`?**
  _High betweenness centrality (0.045) - this node is a cross-community bridge._
- **What connects `$schema`, `productName`, `version` to the rest of the system?**
  _272 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Community 0` be split into smaller, more focused modules?**
  _Cohesion score 0.04 - nodes in this community are weakly interconnected._
- **Should `Community 1` be split into smaller, more focused modules?**
  _Cohesion score 0.08 - nodes in this community are weakly interconnected._
- **Should `Community 2` be split into smaller, more focused modules?**
  _Cohesion score 0.05 - nodes in this community are weakly interconnected._