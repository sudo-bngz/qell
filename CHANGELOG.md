# Changelog

All notable changes to Qell. Versions follow [Semantic Versioning](https://semver.org);
0.x releases are previews, and things may still change between them.

## [0.1.0] — unreleased

The first release: a native code editor in Rust (egui), terminal-like and
quiet — nothing is downloaded or run in the background unless you ask.

### Editor

- Multiple cursors, word wrap, minimap, bracket pair colours and indent guides (#4, #6, #8, #11, #21)
- Find and replace in the file, toggle comment, auto-closing brackets and quotes (#14, #22, #23)
- Vim mode with a command line (#12)
- Code folding by indentation, sticky scroll and an Outline section (#59, #64)
- Split editor, side by side, with a cursor per side, saved in the session (#61, #63)
- Markdown live preview (#57)
- Pictures, video and sound open in a viewer (#37)
- Tabs count to their tab stop for clicks, the cursor and wrapping (#62)
- Asks before closing tabs or the window with unsaved changes (#55)
- Notices files changed outside Qell (#19)
- Dead keys and input methods (⌥N for ~ on a Mac) in the editor and terminal (#70)

### Languages

- Syntax highlighting for about twenty languages, alea included; TypeScript on JavaScript's queries (#1, #50)
- Language servers: go to definition, completion, rename, quick fixes, go to symbol (#3, #24, #25, #36)
- Signature help while typing a call, and inlay hints (#65)
- Format on demand and on save; saving never waits on a stuck formatter (#20, #49)
- Problems tab, messages at line end, errors underlined in the file tree (#51, #52, #54)

### Git

- Change marks in the gutter; stage or revert a change from there (#13, #27)
- Blame column (#60)
- Branches: switch (local and remote), create, delete; stash, pop, apply, drop (#66)
- Merge conflicts: Accept Current / Incoming / Both on each block (#67)
- Each repository's branch shown in the file tree (#48)

### Terminal

- Integrated terminals with selection, copy and a right-click menu (#9)
- Shells come back in the folders they were in (#56)
- Draws prompt symbols the bundled fonts lack with the system's fonts (#53)

### Explorer and search

- Search and replace across files (#3, #26)
- Shortcuts to the files and folders you open most (#15, #17)
- Order by name, last modified or use, or show only changes (#45)
- Handles big workspaces with hundreds of repositories (#47)

### Cloud

- Kubernetes: a k9s-style cluster view — describe, edit, logs, shell, port-forward, delete, live CPU and memory (#28–#35)
- AWS: a k9s-style view of Lambda, CloudFront, CodeDeploy, S3, EC2, CodePipeline, CodeBuild, ECS, RDS, IAM, SSM, Step Functions, SQS, Cognito and API Gateway; sign in again with one key (#38–#40, #58)

### Tests and journal

- Run tests from a ▶ in the gutter (Go, Rust, Jest, Vitest, pytest), results in a Tests tab (#68)
- Daily log: Qell keeps a local record of your day and an LLM of your choice writes a stand-up summary (#69)

### Look

- A terminal look in near-black and white, the syntax colours kept, file icons in colour (#42, #43, #46)
- Native menu bar on macOS; right-click menus in the tree, editor and tabs (#2, #10)
- Zoom scales the whole window (#44)

### Reliability

- Session restore: tabs, cursors, the split and the terminals' folders (#16, #56, #63)
- Crash reports; a broken frame no longer closes the window; unsaved tabs are backed up and put back after a crash (#71)
- Release builds: a universal macOS app and a Linux archive (#72), with the LED logo as the app icon (#74)
