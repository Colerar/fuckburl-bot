# fuckburl-bot

Clean tracking parameters, expand short links, and rewrite shared URLs using TOML rules. Text and QR codes use the same dispatcher.

## Build

```shell
cargo build --release
```

## Usage

```plaintext
Usage: fuckburl-bot [OPTIONS]

Options:
  -c, --config-file <FILE>
  -v, --verbose...         More output per occurrence
  -q, --quiet...           Less output per occurrence
  -h, --help               Print help information
```

Run `fuckburl-bot` to generate `config.toml` and a separate `replacer.toml` in the working directory (`work_dir/` in debug builds), or choose the main configuration path with `--config-file`. On the first run the bot creates the files and exits so you can configure it.

The [main configuration](src/config.example.toml) contains the Telegram settings:

```toml
telegram-token = "your-token"
enabled-chats = ["group_name", "-10011231232"]
```

Rules default to `replacer.toml` next to the main configuration, so no rule-file setting is needed. To use another location, set `replacer-file = "rules/custom.toml"`; relative paths are resolved from the main configuration's directory, and absolute paths are also supported. Missing rule files are populated with defaults at startup.

The [rule configuration](src/replacer.example.toml) uses top-level `[[rules]]` entries. For example, this complete rule file retains useful Bilibili video parameters and rewrites X status links:

```toml
[[rules]]
hosts = ["bilibili.com", "www.bilibili.com"]
path = "/video/{id}"
keep-query = ["p", "t"]

[[rules]]
hosts = ["x.com", "www.x.com", "twitter.com"]
path = "/{user}/status/{id}"
rewrite-scheme = "https"
rewrite-host = "fixupx.com"
keep-query = []
```

## Hot reload

Both files are monitored by comparing their contents every 500 ms. Changes are applied after two identical reads, typically within a second, including saves that replace the file by renaming a temporary file. No restart is needed.

The main configuration and referenced rules are parsed and validated together before publishing an immutable snapshot. Invalid TOML, invalid rules, client configuration errors, and missing files leave the last valid snapshot active and log an error. Restoring a valid file resumes reloading automatically. Files deleted while running are not regenerated.

Changes to chats, polling delays, the token, the proxy, and the rule-file path take effect together. An ongoing Telegram long poll or retry delay is interrupted when a new snapshot is published. Client settings are rebuilt, and changing the bot token resets its update offset. Messages already being processed retain their original snapshot for both text and QR codes.

## Replacer rules

URLs are parsed with `url::Url` and dispatched by their exact, case-insensitive host. Subdomains must be listed explicitly. Within a host, the first matching rule wins; ordinary rewrites run once. Unmatched URLs remain unchanged. HTTP(S) links and configured bare domains are supported, including multiple links in one message.

URLs can contain raw Unicode in their hostname, path, query, and fragment, including a mix of percent-encoded and unencoded characters. Configured Unicode hostnames use IDNA normalization for matching. Bare URLs such as `example.com/中文?q=你好` and protocol-relative links such as `//example.com/中文` default to HTTPS. Extraction retains the original UTF-8 byte spans; rewritten URLs use the URL parser's normalized serialization. Balanced Unicode brackets and punctuation inside URLs are preserved, while trailing sentence punctuation and unmatched closing brackets remain outside the URL.

| Field | Behavior |
| --- | --- |
| `hosts` | Required list of hostnames, without a scheme, port, or path. |
| `path` | Optional whole-path pattern; omitted matches any path. `{name}` captures one nonempty segment, `*` matches one nonempty segment, and `**` matches zero or more segments. A final slash is optional. |
| `rewrite-scheme` | Set `http` or `https`. |
| `rewrite-host` | Replace the hostname, preserving the port. |
| `rewrite-path` | Replace the path; captured names can be interpolated, e.g. `/read/cv{id}`. |
| `keep-query` | Keep only the listed keys, preserving parameter order and repeated values. `[]` removes all parameters; omitting the field preserves the query. Fragments are preserved. |
| `resolve-redirect` | Follow HTTP redirects before applying rewrites and filtering the query, then dispatch the destination again. Captures refer to the original matched path. Defaults to `false`. |

Short-link requests use the configured proxy, a 10-second connection timeout, a 30-second request timeout, and at most five HTTP redirects per request. Dispatch loops are bounded to eight passes. Invalid initial configuration fails at startup before the bot connects to Telegram.

The separate rule file defines the entire rule set. To disable replacements, replace its contents with:

```toml
rules = []
```

## Validation

```shell
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
```

The tests cover default and custom rules, direct URL extraction with raw Unicode and bare domains, UTF-8 byte boundaries, multiple URLs, QR codes, redirects through a local HTTP server, initial file generation, hot reload, atomic saves, invalid-file recovery, and rule-file path changes. They do not require external network access.

`rstest` provides fixtures and named cases; `googletest` provides matchers and assertion diagnostics. Error cases check the expected cause. Reload behavior is tested by driving individual scans and checking their outcomes and retained snapshots, with a background-watcher integration test for atomic file saves. Timeouts bound integration tests rather than serve as evidence that a scan completed.
