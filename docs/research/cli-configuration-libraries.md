# CLI parsing and configuration loading

Researched 2026-10-27, covering library selection, CLI/configuration integration, maintenance and adoption. The user subsequently approved clap plus config-rs, now implemented in the `cli` and `config` modules.

## Revised conclusion

**The libraries already implement precedence; soze should not hand-write field-by-field merging.** Bare clap plus config-rs remains a sound baseline, but the initial recommendation underexplored integration. The question is which module owns the declaration, source discovery and binding between argument names and configuration keys.

With the user’s hard criteria of **active maintenance, community support and verifiable real-application use**, the selected libraries are **clap plus config-rs (`config`)**. Both have recent development and named application evidence below. Direct extensions remain feature candidates, but production adoption was not verified for them; do not recommend them under this criterion merely because their integration is convenient.

## Maintenance and real-application evidence

- **clap:** version 4.6.7 shipped 2026-09-14; its default branch has a [2026-10-06 commit](https://github.com/clap-rs/clap/commit/ccc89b83875474399e9d9d70841163f6319d1a41). The current official manifests for [fd](https://github.com/sharkdp/fd/blob/master/Cargo.toml) and [bat](https://github.com/sharkdp/bat/blob/master/Cargo.toml) declare clap 4.6.1. This verifies maintained application use rather than inferring it from popularity.
- **config-rs:** version 0.15.27 shipped 2026-09-30; its default branch has a [2026-10-06 commit](https://github.com/rust-cli/config-rs/commit/ebde4917663d27a6c79288afd3802d785bd7022e). [git-cliff’s actual loader](https://github.com/orhun/git-cliff/blob/main/git-cliff-core/src/config.rs) combines embedded defaults, a TOML file and environment variables using config-rs. [Atuin’s settings implementation](https://github.com/atuinsh/atuin/blob/main/crates/atuin-client/src/settings.rs) also uses its file/environment builders; the [client manifest](https://github.com/atuinsh/atuin/blob/main/crates/atuin-client/Cargo.toml) declares both clap and config.
- Community reach corroborates those examples: the registry reported 49,764 published reverse dependencies for clap and 1,670 for config on 2026-10-27. These are package relationships, not deployment counts. [Clap registry relationships](https://crates.io/api/v1/crates/clap/reverse_dependencies?page=1&per_page=1), [config registry relationships](https://crates.io/api/v1/crates/config/reverse_dependencies?page=1&per_page=1).

Recent development alone does not settle adoption. Production use of the smaller integrated extensions below remains **unverified in this research**, rather than proven absent. Figment has substantial ecosystem use and recent commits despite its older release; it is not classified as abandoned.

## Integrated candidates

Production adoption remains unverified for these extensions; this table compares features, not approved choices.

| Candidate | Verified fit and limitation |
| --- | --- |
| **clap-config-fallback** | One declaration; CLI > environment > config > default; required values can come from files; fallible parsing. Loads one config path. Argument reconstruction needs behavior tests. [Project](https://github.com/alekece/clap-config-fallback), [runtime source](https://docs.rs/crate/clap-config-fallback/0.1.13/source/src/lib.rs) |
| **clapfig** | Multi-file discovery, environment inputs, configuration commands/templates and automatic `cli_overrides_from` key matching. Still requires sparse CLI values beside its schema. [Official documentation](https://docs.rs/clapfig/latest/clapfig/) |
| **conf** | Unified declaration with CLI/environment/document/default resolution, post-source requiredness and injected args/environment. Accepts one preloaded document; rejects repeated scalar arguments used by today's dev script. [Builder](https://docs.rs/conf/latest/conf/struct.ConfBuilder.html), [parser source](https://docs.rs/crate/conf/0.4.5/source/src/conf_context.rs) |
| **twelf** | Generates clap arguments and merges ordered sources while respecting clap default provenance. Environment reads process state; generated code debug-logs all merged settings. [Docs](https://docs.rs/twelf/latest/twelf/), [generator](https://docs.rs/crate/config-derive/0.15.0/source/src/lib.rs) |
| **clap-layers** | Direct derive extension with source precedence. Documented gaps: required fields fail before lower sources fill them; unknown file keys are ignored. [Docs](https://docs.rs/clap-layers/latest/clap_layers/) |
| **clap_config** | Generates optional config fields and merging. Its published README lists config-provided required/positional fields as unfinished. [README](https://docs.rs/crate/clap_config/0.1.3/source/README.md) |
| **clap-serde-derive** | Generates optional clap/Serde proxy fields and merging. Defaults, file loading and required-after-merge policy still need an owner. [Docs](https://docs.rs/clap-serde-derive/latest/clap_serde_derive/) |
| **clap-config-file** | Single derive and walk-up discovery, but directly prints/exits, lacks environment loading and defaults missing ordinary nonoptional fields. [Docs](https://docs.rs/clap-config-file), [generator](https://docs.rs/crate/clap-config-file/0.6.0/source/src/lib.rs) |

Conf resolves source priority before field parsing and checks required values after clap; omitted flags need not erase document values. Its README delegates multiple-file loading to a library such as Figment; original per-file metadata does not survive as Conf provenance. [Own hierarchical-config documentation](https://docs.rs/conf/latest/conf/readme/index.html#hierarchical-config).

## Baseline libraries

Clap supplies typed argument declarations, help, validation and value-source provenance. Config-rs supplies ordered sources, defaults and explicit overrides; `set_override_option` ignores absent overrides, and `Environment::source` supports synthetic environment maps. Figment supplies provider composition, typed extraction and rich provenance. A bridge can bind those existing capabilities; an application-written merge algorithm is unnecessary. [Clap provenance](https://docs.rs/clap/4.6.7/clap/parser/enum.ValueSource.html), [config builder](https://docs.rs/config/latest/config/builder/struct.ConfigBuilder.html), [config environment](https://docs.rs/config/latest/config/struct.Environment.html#method.source), [Figment](https://docs.rs/figment/latest/figment/).

## Age-eligible versions

All versions below are non-yanked registry releases at least fourteen days old as checked on 2026-10-27. For a release dated 2026-10-13, the publication time was checked against the current UTC time minus fourteen days. Exact pins are proposals, not manifest edits.

| Crate | Pin | Published |
| --- | --- | --- |
| clap | `=4.6.7` | 2026-09-14 |
| config | `=0.15.27` | 2026-09-30 |
| figment | `=0.10.19` | 2024-05-17 |
| clapfig | `=0.26.0` | 2026-09-03 |
| clap-config-fallback | `=0.1.13` | 2026-10-13 07:17 UTC |
| clap_config | `=0.1.3` | 2026-10-01 |
| conf | `=0.4.5` | 2025-12-17 |
| twelf | `=0.15.0` | 2024-03-11 |
| clap-layers | `=0.1.0` | 2026-07-17 |
| clap-serde-derive | `=0.2.1` | 2023-10-19 |
| clap-config-file | `=0.6.0` | 2025-09-23 |

Direct-extension versions were checked against published archives: [clap-config-fallback](https://crates.io/api/v1/crates/clap-config-fallback/0.1.13), [clap_config](https://crates.io/api/v1/crates/clap_config/0.1.3).

Verified from first-party registry records: [clap](https://crates.io/api/v1/crates/clap/4.6.7), [config](https://crates.io/api/v1/crates/config/0.15.27), [Figment](https://crates.io/api/v1/crates/figment/0.10.19), [clapfig](https://crates.io/api/v1/crates/clapfig/0.26.0), [conf](https://crates.io/api/v1/crates/conf/0.4.5), [twelf](https://crates.io/api/v1/crates/twelf/0.15.0), [clap-layers](https://crates.io/api/v1/crates/clap-layers/0.1.0), [clap-serde-derive](https://crates.io/api/v1/crates/clap-serde-derive/0.2.1), [clap-config-file](https://crates.io/api/v1/crates/clap-config-file/0.6.0).

If choosing config-rs directly, disable default features and select only TOML. Never enable `config/preserve_order`: it can enable `serde_json/preserve_order` and violate sorted tap serialization. Clapfig currently carries several unconditional format/support dependencies; no binary-size or compile-time comparison was measured. [Config features](https://github.com/rust-cli/config-rs/blob/main/Cargo.toml), [clapfig dependencies](https://crates.io/api/v1/crates/clapfig/0.26.0/dependencies).

## Soze integration checks

The roadmap requests layered defaults/system/user/environment/flags plus validation, versioning and reload. Choose library-owned precedence, then keep soze-specific validation and publication behind its configuration module interface:

- Validate required `tap_file` after sources resolve; check debug/capture path aliases before truncation.
- Preserve explicit logging level over the `--stream` default and calculate terminal formatting beneath overrides.
- Read only declared environment inputs; currently the binary reads `SOZE_LOG` and `SOZE_LOG_FORMAT`, while wrappers own other `SOZE_` names.
- Preserve `scripts/dev.sh` user overrides. Conf requires changing how the script supplies defaults, rather than assuming repeated scalar flags work.
- Use fallible parsing to control bounded stderr behavior. Test injected arguments/environment, absent booleans, required values, help/version, repeats and invalid lower-priority values.
- Determine reloadable settings; load/validate a candidate off the forwarding path and publish it only on success. File watching, migrations and last-known-good runtime publication are not established merely by adopting a loader. [Config watch example](https://github.com/rust-cli/config-rs/blob/main/examples/watch.rs).

Evidence for current behavior: `src/cli.rs`, `scripts/dev.sh`, `README.md` and `docs/roadmap.md`. The existing parser validates logging environment values before CLI overrides; changing that error precedence must be deliberate. Library errors and provenance must not expose raw configuration values or arbitrary paths in operational logs.
