//! 扩展监管：把 spec ch1–ch9 的应用层那一半落成一个可以真起进程的运行时。
//!
//! 这不是一份"接口草稿"：每一项能力都对应附录 A 里至少一条判据，且判据只读这里产生的
//! 观测（pid / fd / environ / 帧 / 提交）。所以这里**不打印期望值**，只产生事实。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nous_ext_sdk::content::Handles;
use nous_ext_sdk::frame::PROTOCOL;
use nous_ext_sdk::manifest::Manifest;
use serde_json::{json, Value};

use crate::audit::{Audit, AuditEvent, Provenance, ToolReceipt};
use crate::config::RuntimeConfig;
use crate::error::{ExtError, ExtResult, FailureClass};
use crate::handle;
use crate::manifest::{self, Artifact, LoadedManifest};
use crate::nameplate::Nameplates;
use crate::spawn::{ProcExt, SpawnSpec};
use crate::state::{Diagnostics, ExtState};

/// 一次状态转换的推送（熔断必须推到客户端，spec 4.3）。
#[derive(Debug, Clone)]
pub struct StateCommit {
    pub conversation: i64,
    pub entry_id: i64,
    pub commit_seq: i64,
    pub name: String,
    pub event: AuditEvent,
    pub state: String,
}

/// 推送钩子：拿到一条已落库的 `pi.extension` 提交。
pub type PushHook = Arc<dyn Fn(&StateCommit) + Send + Sync>;

/// 一个已装载的扩展。
pub struct Extension {
    pub loaded: LoadedManifest,
    pub artifact: Artifact,
    pub process: Option<Arc<ProcExt>>,
    pub state: ExtState,
    /// 装载时代读的内容。
    pub content: Handles,
    pub nameplates: Vec<String>,
    /// 上一轮渲染出来的段文本（崩溃时保留，零增量）。
    pub shown: String,
    pub crash_count: u32,
    /// 装载的版本（重载后变）。
    pub version: String,
    pub uid: Option<u32>,
}

impl Extension {
    pub fn name(&self) -> &str {
        &self.loaded.manifest.name
    }
}

/// 一轮 section 装配的产物。
#[derive(Debug, Clone, Default)]
pub struct Turn {
    /// 拼好的系统提示词（按清单顺序）。
    pub text: String,
    /// 每个扩展贡献的文本（按装载顺序）。
    pub per_ext: Vec<(String, String)>,
    /// 每个扩展这一轮被调了几次（EX-09）。
    pub calls: BTreeMap<String, u32>,
}

/// 一次工具调用的产物。
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub call_id: u64,
    pub receipt: Option<ToolReceipt>,
    pub lines: Vec<String>,
    /// 终帧 `result.content`（工具自己的文本）。
    pub result_text: String,
    pub cancelled: bool,
    /// 被拒的晚期输出（EX-19）。
    pub rejected_late_output: bool,
    pub replayed: bool,
}

/// 并发闸（16 在飞 / 超出排队并点名，EX-34）。
struct Gate {
    inflight: usize,
    queued: usize,
    max: usize,
    last_queue_message: Option<String>,
}

struct GateGuard<'a> {
    runtime: &'a Runtime,
    released: bool,
}

impl GateGuard<'_> {
    fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut gate = self.runtime.gate.lock().expect("gate");
        gate.inflight = gate.inflight.saturating_sub(1);
    }
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

/// 运行时。
pub struct Runtime {
    pub config: RuntimeConfig,
    exts: Mutex<BTreeMap<String, Arc<Mutex<Extension>>>>,
    order: Mutex<Vec<String>>,
    pub nameplates: Mutex<Nameplates>,
    pub diagnostics: Mutex<Diagnostics>,
    audit: Option<Arc<Mutex<Audit>>>,
    push: Option<PushHook>,
    call_counter: AtomicU64,
    uid_counter: AtomicU32,
    gate: Mutex<Gate>,
    /// 最近的 host→ext 帧（EX-08 / EX-12 直接读它）。
    last_frames: Mutex<Vec<Value>>,
    /// 每个扩展的重载次数（EX-64 / EX-65）。
    reloads: Mutex<BTreeMap<String, u32>>,
    /// watch 的待装载队列：扩展名 → 第一次看到变化的时间。
    pending: Mutex<BTreeMap<String, u64>>,
    /// 上一轮看到的变化签名。
    signatures: Mutex<BTreeMap<String, String>>,
    /// 已取消的调用（晚期输出要拒，EX-19）。
    cancelled: Mutex<BTreeSet<u64>>,
    /// 本次启动的 id（重启后变，EX-61）。
    boot_id: u64,
    /// 审计 entry 住在哪个会话（push 时**不能**再锁审计库：调用方正拿着锁）。
    audit_conversation: i64,
}

impl Runtime {
    /// 建一个运行时。**不启动扩展**；`start_all()` 才是装载。
    pub fn new(config: RuntimeConfig) -> Arc<Self> {
        let audit = config.db.as_ref().map(|path| {
            Arc::new(Mutex::new(Audit::open(path).expect("open audit db")))
        });
        let audit_conversation = audit
            .as_ref()
            .map(|audit| audit.lock().expect("audit").conversation)
            .unwrap_or(1);
        let max = config.limits.concurrency;
        Arc::new(Self {
            config,
            exts: Mutex::new(BTreeMap::new()),
            order: Mutex::new(Vec::new()),
            nameplates: Mutex::new(Nameplates::default()),
            diagnostics: Mutex::new(Diagnostics::new(0)),
            audit,
            push: None,
            call_counter: AtomicU64::new(1001),
            uid_counter: AtomicU32::new(0),
            gate: Mutex::new(Gate {
                inflight: 0,
                queued: 0,
                max,
                last_queue_message: None,
            }),
            last_frames: Mutex::new(Vec::new()),
            reloads: Mutex::new(BTreeMap::new()),
            pending: Mutex::new(BTreeMap::new()),
            signatures: Mutex::new(BTreeMap::new()),
            cancelled: Mutex::new(BTreeSet::new()),
            boot_id: now_ms(),
            audit_conversation,
        })
    }

    /// 带推送钩子的构造。
    pub fn new_with_push(config: RuntimeConfig, push: Option<PushHook>) -> Arc<Self> {
        let audit = config.db.as_ref().map(|path| {
            Arc::new(Mutex::new(Audit::open(path).expect("open audit db")))
        });
        let audit_conversation = audit
            .as_ref()
            .map(|audit| audit.lock().expect("audit").conversation)
            .unwrap_or(1);
        let max = config.limits.concurrency;
        let dedup = config.limits.diag_dedup_ms;
        Arc::new(Self {
            config,
            exts: Mutex::new(BTreeMap::new()),
            order: Mutex::new(Vec::new()),
            nameplates: Mutex::new(Nameplates::default()),
            diagnostics: Mutex::new(Diagnostics::new(dedup)),
            audit,
            push,
            call_counter: AtomicU64::new(1001),
            uid_counter: AtomicU32::new(0),
            gate: Mutex::new(Gate {
                inflight: 0,
                queued: 0,
                max,
                last_queue_message: None,
            }),
            last_frames: Mutex::new(Vec::new()),
            reloads: Mutex::new(BTreeMap::new()),
            pending: Mutex::new(BTreeMap::new()),
            signatures: Mutex::new(BTreeMap::new()),
            cancelled: Mutex::new(BTreeSet::new()),
            boot_id: now_ms(),
            audit_conversation,
        })
    }

    pub fn set_clock_dedup(&self, dedup_ms: u64) {
        *self.diagnostics.lock().expect("diag") = Diagnostics::new(dedup_ms);
    }

    /// 全部扩展名（装载顺序）。
    pub fn names(&self) -> Vec<String> {
        self.order.lock().expect("order").clone()
    }

    pub fn extension(&self, name: &str) -> Option<Arc<Mutex<Extension>>> {
        self.exts.lock().expect("exts").get(name).cloned()
    }

    pub fn state_of(&self, name: &str) -> Option<ExtState> {
        self.extension(name)
            .and_then(|ext| ext.lock().ok().map(|ext| ext.state))
    }

    /// 装载所有扩展。**一个扩展坏了不许把宿主拖死**（EX-66）：其余照常，进程不退。
    pub fn start_all(&self) -> BTreeMap<String, ExtResult<()>> {
        let mut results = BTreeMap::new();
        for (dir, loaded) in manifest::scan(&self.config.exts_root) {
            let loaded = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    let name = dir
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| dir.display().to_string());
                    if let Some(audit) = &self.audit {
                        let mut audit = audit.lock().expect("audit");
                        let _ = audit.commit_event(
                            AuditEvent::Breaker,
                            &name,
                            "load_failed",
                            "",
                            &[],
                            None,
                        );
                    }
                    results.insert(name, Err(error));
                    continue;
                }
            };
            let name = loaded.manifest.name.clone();
            let hash = loaded.hash.clone();
            let handles = loaded.manifest.handles.clone();
            let result = self.start_one(loaded);
            if let Err(error) = &result {
                // 开机装载失败必须可见（EX-66）：落一条提交，其余扩展照常，进程不退。
                eprintln!("nous-ext: {name}: {error}");
                if let Some(audit) = &self.audit {
                    let mut audit = audit.lock().expect("audit");
                    let _ = audit.commit_event(
                        AuditEvent::Breaker,
                        &name,
                        "load_failed",
                        &hash,
                        &handles,
                        None,
                    );
                }
            }
            results.insert(name, result);
        }
        results
    }

    /// 装载一个扩展：校验产物 → 代读内容 → spawn → 握手 → 装名牌。
    pub fn start_one(&self, loaded: LoadedManifest) -> ExtResult<()> {
        let name = loaded.manifest.name.clone();
        let artifact = manifest::resolve_artifact(&self.config, &loaded.manifest)?;

        // 缺句柄 ⇒ 拒载并指名（EX-02）。
        let delivery = handle::deliver(
            &loaded.manifest.handles,
            &self.config.handles,
            self.config.limits.handle_bytes,
        )
        .map_err(ExtError::MissingHandle)?;
        for note in &delivery.diagnostics {
            eprintln!("nous-ext: {name}: {note}");
        }
        if !delivery.diagnostics.is_empty() {
            self.diagnostics.lock().expect("diag").record(
                &name,
                "handle",
                "read",
                &delivery.diagnostics.join("; "),
                now_ms(),
            );
        }

        let process = self.spawn_one(&loaded, &artifact, &delivery.handles)?;
        let names = process
            .handshake(
                PROTOCOL,
                &loaded.hash,
                &delivery.handles,
                Duration::from_millis(self.config.limits.handshake_timeout_ms),
            )
            .map_err(|error| {
                process.kill();
                error
            })?;

        // 名牌冲突：后装载的拒载并指名双方，先装的照常工作（EX-59）。
        if let Err(conflict) = self
            .nameplates
            .lock()
            .expect("nameplates")
            .install(&name, &names, false)
        {
            process.kill();
            return Err(ExtError::NameplateConflict {
                name: conflict.name,
                winner: conflict.winner,
                loser: conflict.loser,
            });
        }

        let mut extension = Extension {
            version: loaded.manifest.version.clone(),
            uid: process.uid,
            loaded,
            artifact,
            process: Some(process),
            state: ExtState::Normal,
            content: delivery.handles,
            nameplates: names,
            shown: String::new(),
            crash_count: 0,
        };
        // 重启后状态来自提交（计数归零，状态仍在，EX-61）。
        if let Some(audit) = &self.audit {
            let audit = audit.lock().expect("audit");
            let states = audit.latest_states().unwrap_or_default();
            if let Some((_, state)) = states.into_iter().find(|(ext, _)| ext == &name) {
                if let Some(state) = ExtState::parse(&state) {
                    extension.state = state;
                }
            }
        }

        let name_for_audit = extension.name().to_string();
        let hash = extension.loaded.hash.clone();
        let handles = extension.loaded.manifest.handles.clone();
        let provenance = Provenance {
            hash: extension.artifact.meta.hash.clone(),
            built_from: extension.artifact.meta.built_from.clone(),
            build_machine: extension.artifact.meta.build_machine.clone(),
        };
        let state = extension.state;

        self.exts
            .lock()
            .expect("exts")
            .insert(name.clone(), Arc::new(Mutex::new(extension)));
        self.order.lock().expect("order").push(name.clone());

        // 审计：装（含清单哈希、授予的句柄、来源三项）。
        if let Some(audit) = &self.audit {
            let mut audit = audit.lock().expect("audit");
            if let Ok(commit) =
                audit.commit_event(AuditEvent::Install, &name_for_audit, state.as_str(), &hash, &handles, Some(&provenance))
            {
                self.push_state(&name_for_audit, AuditEvent::Install, state, commit);
            }
        }
        Ok(())
    }

    fn spawn_one(
        &self,
        loaded: &LoadedManifest,
        artifact: &Artifact,
        content: &Handles,
    ) -> ExtResult<Arc<ProcExt>> {
        let uid = if self.config.isolation.enabled {
            let index = self.uid_counter.fetch_add(1, Ordering::SeqCst);
            Some(self.config.isolation.base_uid + index)
        } else {
            None
        };
        let spec = SpawnSpec {
            program: artifact.path.clone(),
            ext_dir: loaded.dir.clone(),
            uid,
            isolation: self.config.isolation.clone(),
            rss_limit: self.config.limits.rss_bytes,
        };
        let process = ProcExt::spawn(&spec)?;
        let _ = content;
        Ok(Arc::new(process))
    }

    fn push_state(&self, name: &str, event: AuditEvent, state: ExtState, commit: (i64, i64)) {
        if let Some(hook) = &self.push {
            hook(&StateCommit {
                conversation: self.audit_conversation,
                entry_id: commit.0,
                commit_seq: commit.1,
                name: name.to_string(),
                event,
                state: state.as_str().to_string(),
            });
        }
    }

    /// 记一次失败并推进状态机。返回是否发生了状态转换（转换才落提交，spec 4.2）。
    pub fn note_failure(&self, name: &str, segment: &str, class: FailureClass) -> Option<ExtState> {
        let now = now_ms();
        let logged = self.diagnostics.lock().expect("diag").record(
            name,
            segment,
            class.as_str(),
            class.as_str(),
            now,
        );
        if logged {
            eprintln!("nous-ext: {name}/{segment}: {class}");
        }

        let Some(arc) = self.extension(name) else {
            return None;
        };
        let mut ext = arc.lock().expect("ext");
        let before = ext.state;
        if !before.callable() {
            return None;
        }
        let transition = if class.is_crash() {
            ext.crash_count += 1;
            // 重启一次：第一次崩 = degraded；第二次崩 = 进程熔断（无自动恢复）。
            if ext.crash_count >= 2 {
                ext.state = ExtState::TrippedProcess;
            } else {
                ext.state = ExtState::Degraded;
            }
            if let Some(process) = ext.process.take() {
                process.kill();
            }
            true
        } else {
            let threshold = self.config.limits.failure_threshold;
            let reached = self
                .diagnostics
                .lock()
                .expect("diag")
                .reached(name, segment, threshold);
            if reached {
                ext.state = ExtState::TrippedSection;
                true
            } else {
                false
            }
        };
        if !transition || ext.state == before {
            return None;
        }
        let state = ext.state;
        let hash = ext.loaded.hash.clone();
        let handles = ext.loaded.manifest.handles.clone();
        drop(ext);

        if let Some(audit) = &self.audit {
            let mut audit = audit.lock().expect("audit");
            if state == ExtState::TrippedSection {
                let _ = audit.commit_section_removal(name, segment);
            }
            if let Ok(commit) = audit.commit_event(
                AuditEvent::Breaker,
                name,
                state.as_str(),
                &hash,
                &handles,
                None,
            ) {
                self.push_state(name, AuditEvent::Breaker, state, commit);
            }
        }
        Some(state)
    }

    /// 一次 section 装配：并发取段，整轮合计 1s 预算。
    pub fn render_turn(&self, conversation: i64, generation: i64, cwd: &str) -> Turn {
        let _ = generation;
        let names = self.names();
        let deadline = Instant::now() + Duration::from_millis(self.config.limits.section_budget_ms);
        let (tx, rx) = std::sync::mpsc::channel::<(String, ExtResult<Vec<Value>>)>();
        let mut calls = BTreeMap::new();

        std::thread::scope(|scope| {
            for name in &names {
                let Some(arc) = self.extension(name) else {
                    continue;
                };
                let (state, process, section_decls) = {
                    let ext = arc.lock().expect("ext");
                    (
                        ext.state,
                        ext.process.clone(),
                        ext.loaded.manifest.sections.clone(),
                    )
                };
                if !state.callable() || section_decls.is_empty() {
                    continue;
                }
                *calls.entry(name.clone()).or_insert(0) += 1;
                // 段熔断：该段不出现，其余段照常。
                if state == ExtState::TrippedSection {
                    let _ = tx.send((name.clone(), Ok(Vec::new())));
                    continue;
                }
                let _ = process;
                let sender = tx.clone();
                let name = name.clone();
                scope.spawn(move || {
                    let result = match self.ensure_process(&name) {
                        Ok(process) => self.call_section(&name, &process, conversation, cwd, deadline),
                        Err(error) => Err(error),
                    };
                    let _ = sender.send((name, result));
                });
            }
        });
        drop(tx);

        let mut turn = Turn {
            calls,
            ..Default::default()
        };
        for (name, result) in rx {
            match result {
                Ok(sections) => {
                    let text = self.render_sections(&name, &sections);
                    if let Some(arc) = self.extension(&name) {
                        if let Ok(mut ext) = arc.lock() {
                            ext.shown = text.clone();
                        }
                    }
                    turn.per_ext.push((name, text));
                }
                Err(error) => {
                    let segment = self.segment_of(&name);
                    let class = match error {
                        ExtError::HardLimit { class, .. } => class,
                        ExtError::ProtocolMismatch { .. } => FailureClass::Crash,
                        _ => FailureClass::Timeout,
                    };
                    self.note_failure(&name, &segment, class);
                    // 失败的那一轮：保留 shown，零增量。
                    let shown = self
                        .extension(&name)
                        .and_then(|arc| arc.lock().ok().map(|ext| ext.shown.clone()))
                        .unwrap_or_default();
                    turn.per_ext.push((name, shown));
                }
            }
        }
        turn.text = turn
            .per_ext
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        turn
    }

    fn segment_of(&self, name: &str) -> String {
        self.extension(name)
            .and_then(|arc| {
                arc.lock().ok().map(|ext| {
                    ext.loaded
                        .manifest
                        .sections
                        .first()
                        .map(|section| section.key.clone())
                        .unwrap_or_else(|| "section".to_string())
                })
            })
            .unwrap_or_else(|| "section".to_string())
    }

    /// 崩一次之后**自动重启一次**（spec 4.0）。第二次崩由状态机拦下。
    fn ensure_process(&self, name: &str) -> ExtResult<Arc<ProcExt>> {
        let arc = self
            .extension(name)
            .ok_or_else(|| ExtError::Spawn(format!("unknown extension {name}")))?;
        let existing = {
            let ext = arc.lock().expect("ext");
            if !ext.state.callable() {
                return Err(ExtError::HardLimit {
                    class: FailureClass::Error,
                    detail: format!("reason=state={}", ext.state.as_str()),
                });
            }
            ext.process.clone()
        };
        if let Some(process) = existing {
            if !process.flags.crashed.load(Ordering::SeqCst) {
                return Ok(process);
            }
        }
        let (loaded, artifact, content) = {
            let mut ext = arc.lock().expect("ext");
            ext.process = None;
            (
                ext.loaded.clone(),
                ext.artifact.clone(),
                ext.content.clone(),
            )
        };
        let process = self.spawn_one(&loaded, &artifact, &content)?;
        let names = process.handshake(
            PROTOCOL,
            &loaded.hash,
            &content,
            Duration::from_millis(self.config.limits.handshake_timeout_ms),
        )?;
        let _ = self.nameplates.lock().expect("nameplates").install(name, &names, false);
        let mut ext = arc.lock().expect("ext");
        ext.process = Some(process.clone());
        ext.nameplates = names;
        Ok(process)
    }

    fn call_section(
        &self,
        name: &str,
        process: &Arc<ProcExt>,
        conversation: i64,
        cwd: &str,
        deadline: Instant,
    ) -> ExtResult<Vec<Value>> {
        let call = self.next_call();
        let rx = process.register(call);
        let frame = json!({
            "call": call,
            "kind": "section",
            "input": {
                "conversationId": conversation,
                "cwd": cwd,
                "extensions": self.names(),
            }
        });
        self.record_frame(&frame);
        process.send(&frame)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = match rx.recv_timeout(remaining) {
            Ok(Ok(response)) => {
                if let Some(error) = response.get("error").and_then(Value::as_str) {
                    Err(ExtError::HardLimit {
                        class: FailureClass::Error,
                        detail: format!("reason={error}"),
                    })
                } else {
                    Ok(response
                        .get("sections")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default())
                }
            }
            Ok(Err(message)) => Err(ExtError::HardLimit {
                class: FailureClass::Crash,
                detail: format!("reason={message}"),
            }),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(ExtError::HardLimit {
                class: FailureClass::Crash,
                detail: "reason=进程死了且没回段".to_string(),
            }),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(ExtError::HardLimit {
                class: FailureClass::Timeout,
                detail: format!("reason=section budget {name}"),
            }),
        };
        process.unregister(call);
        result
    }

    /// 按**清单**的顺序与 `tag` 渲染（应答里的顺序与 `tag` 一律不算数）。
    fn render_sections(&self, name: &str, reply: &[Value]) -> String {
        let Some(arc) = self.extension(name) else {
            return String::new();
        };
        let ext = arc.lock().expect("ext");
        let by_key: BTreeMap<&str, &Value> = reply
            .iter()
            .filter_map(|section| {
                section
                    .get("key")
                    .and_then(Value::as_str)
                    .map(|key| (key, section))
            })
            .collect();
        let mut text = String::new();
        for section in &ext.loaded.manifest.sections {
            if let Some(value) = by_key.get(section.key.as_str()) {
                if let Some(chunk) = value.get("text").and_then(Value::as_str) {
                    text.push_str(chunk);
                    if !chunk.ends_with('\n') {
                        text.push('\n');
                    }
                }
            }
        }
        text
    }

    fn next_call(&self) -> u64 {
        self.call_counter.fetch_add(1, Ordering::SeqCst)
    }

    fn record_frame(&self, frame: &Value) {
        if let Ok(mut frames) = self.last_frames.lock() {
            frames.push(frame.clone());
            if frames.len() > 256 {
                frames.remove(0);
            }
        }
    }

    /// 最近的 host→ext 帧（判据直接读）。
    pub fn frames(&self) -> Vec<Value> {
        self.last_frames.lock().expect("frames").clone()
    }

    /// 调一次扩展工具（流式 + 取消 + 输出硬顶 + 并发闸）。
    pub fn call_tool(
        &self,
        name: &str,
        tool: &str,
        args: Value,
        conversation: i64,
    ) -> ExtResult<ToolOutcome> {
        let mut guard = self.enter_gate(name);
        let result = self.call_tool_inner(name, tool, args, conversation);
        guard.release();
        result
    }

    fn call_tool_inner(
        &self,
        name: &str,
        tool: &str,
        args: Value,
        conversation: i64,
    ) -> ExtResult<ToolOutcome> {
        let arc = self
            .extension(name)
            .ok_or_else(|| ExtError::UnknownTool(name.to_string()))?;
        let process = self.ensure_process(name)?;
        let _ = &arc;

        let call_id = self.next_call();
        let rx = process.register(call_id);
        let cwd = self
            .extension(name)
            .and_then(|arc| arc.lock().ok().map(|ext| ext.loaded.dir.display().to_string()))
            .unwrap_or_default();
        let frame = json!({
            "call": call_id,
            "kind": "tool",
            "tool": tool,
            "args": args,
            "cwd": cwd,
        });
        self.record_frame(&frame);
        process.send(&frame)?;

        let mut outcome = ToolOutcome {
            call_id,
            receipt: None,
            lines: Vec::new(),
            result_text: String::new(),
            cancelled: false,
            rejected_late_output: false,
            replayed: false,
        };
        let mut bytes = 0usize;
        let mut final_frame: Option<Value> = None;
        let started = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(frame)) => {
                    let kind = frame.get("kind").and_then(Value::as_str).unwrap_or("");
                    match kind {
                        "output" => {
                            let text = frame.get("text").and_then(Value::as_str).unwrap_or("");
                            bytes += text.len();
                            if bytes > self.config.limits.output_bytes {
                                process.kill();
                                process.unregister(call_id);
                                self.note_failure(name, tool, FailureClass::Crash);
                                return Err(ExtError::HardLimit {
                                    class: FailureClass::Crash,
                                    detail: format!(
                                        "detail=单次调用输出 bytes={}",
                                        self.config.limits.output_bytes
                                    ),
                                });
                            }
                            if self.is_cancelled(call_id) {
                                // 取消之后只许发 final（EX-19）。
                                outcome.rejected_late_output = true;
                            } else {
                                outcome.lines.push(text.to_string());
                            }
                        }
                        "final" => {
                            final_frame = Some(frame);
                            break;
                        }
                        _ => {}
                    }
                }
                Ok(Err(message)) => {
                    process.unregister(call_id);
                    self.note_failure(name, tool, FailureClass::Crash);
                    return Err(ExtError::HardLimit {
                        class: FailureClass::Crash,
                        detail: format!("reason={message}"),
                    });
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(limit) = self.config.limits.tool_timeout_ms {
                        if started.elapsed() >= Duration::from_millis(limit) {
                            process.kill();
                            process.unregister(call_id);
                            self.note_failure(name, tool, FailureClass::Timeout);
                            return Err(ExtError::HardLimit {
                                class: FailureClass::Timeout,
                                detail: format!("reason=tool timeout {limit}"),
                            });
                        }
                    }
                    if !process.is_alive() {
                        process.unregister(call_id);
                        self.note_failure(name, tool, FailureClass::Crash);
                        return Err(ExtError::HardLimit {
                            class: FailureClass::Crash,
                            detail: "reason=进程死了且没回终帧".to_string(),
                        });
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    process.unregister(call_id);
                    self.note_failure(name, tool, FailureClass::Crash);
                    return Err(ExtError::HardLimit {
                        class: FailureClass::Crash,
                        detail: "reason=channel closed".to_string(),
                    });
                }
            }
        }
        // 取消之后可能还有一帧 output 在路上：给它一个很短的窗口，再判"拒了"。
        if self.is_cancelled(call_id) {
            let deadline = Instant::now() + Duration::from_millis(80);
            while Instant::now() < deadline {
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(Ok(frame)) if frame.get("kind").and_then(Value::as_str) == Some("output") => {
                        outcome.rejected_late_output = true;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            if process.late_frames(call_id) {
                outcome.rejected_late_output = true;
            }
        }
        process.unregister(call_id);

        let Some(final_frame) = final_frame else {
            self.note_failure(name, tool, FailureClass::Crash);
            return Err(ExtError::HardLimit {
                class: FailureClass::Crash,
                detail: "reason=没有终帧".to_string(),
            });
        };

        if let Some(error) = final_frame.get("error").and_then(Value::as_str) {
            // 明确错误：一条 error 工具结果回给模型，继续（不是崩溃）。
            self.note_failure(name, tool, FailureClass::Error);
            return Err(ExtError::HardLimit {
                class: FailureClass::Error,
                detail: format!("reason={error}"),
            });
        }

        outcome.cancelled = final_frame
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let result = final_frame.get("result").cloned().unwrap_or_else(|| json!({}));
        let text = result
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        outcome.result_text = text.clone();
        let mutations: Vec<Value> = result
            .get("mutations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        if !outcome.cancelled {
            if let Some(audit) = &self.audit {
                let mut audit = audit.lock().expect("audit");
                match audit.commit_tool(conversation, call_id, &text, &mutations) {
                    Ok(receipt) => {
                        outcome.replayed = receipt.replayed;
                        outcome.receipt = Some(receipt);
                    }
                    Err(error) => {
                        // 拒绝的 mutation = 一条 error result 回给模型（EX-22）。
                        self.note_failure(name, tool, FailureClass::Error);
                        return Err(error);
                    }
                }
            }
        }
        self.clear_cancelled(call_id);
        Ok(outcome)
    }

    /// 取消一次调用（走**同一条** host→ext 管道，不是第二个通道）。
    pub fn cancel(&self, name: &str, call_id: u64) -> ExtResult<()> {
        let process = self
            .extension(name)
            .and_then(|arc| arc.lock().ok().and_then(|ext| ext.process.clone()))
            .ok_or_else(|| ExtError::Spawn("no process".into()))?;
        self.cancelled.lock().expect("cancelled").insert(call_id);
        let frame = json!({ "call": call_id, "kind": "cancel" });
        self.record_frame(&frame);
        process.send(&frame)
    }

    fn is_cancelled(&self, call: u64) -> bool {
        self.cancelled.lock().expect("cancelled").contains(&call)
    }

    fn clear_cancelled(&self, call: u64) {
        self.cancelled.lock().expect("cancelled").remove(&call);
    }

    fn enter_gate<'a>(&'a self, name: &str) -> GateGuard<'a> {
        let mut queued = false;
        loop {
            let mut gate = self.gate.lock().expect("gate");
            if gate.inflight < gate.max {
                gate.inflight += 1;
                if queued {
                    gate.queued = gate.queued.saturating_sub(1);
                }
                drop(gate);
                return GateGuard {
                    runtime: self,
                    released: false,
                };
            }
            if !queued {
                gate.queued += 1;
                let message = format!("排队中，等 {} 次", gate.queued);
                gate.last_queue_message = Some(message.clone());
                drop(gate);
                queued = true;
                self.diagnostics.lock().expect("diag").record(
                    name,
                    "tool",
                    "concurrency",
                    &message,
                    now_ms(),
                );
                eprintln!("nous-ext: {name}: {message}");
            } else {
                drop(gate);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// 并发闸的观测（EX-34）。
    pub fn gate_stats(&self) -> (usize, usize, Option<String>) {
        let gate = self.gate.lock().expect("gate");
        (gate.inflight, gate.queued, gate.last_queue_message.clone())
    }

    /// 禁用 / 取消禁用（配置，不是崩溃；下游必须显式失败，EX-76）。
    pub fn set_disabled(&self, name: &str, disabled: bool) -> ExtResult<()> {
        let arc = self
            .extension(name)
            .ok_or_else(|| ExtError::Spawn(format!("unknown extension {name}")))?;
        let (hash, handles) = {
            let mut ext = arc.lock().expect("ext");
            if disabled {
                if let Some(process) = ext.process.take() {
                    process.kill();
                }
                ext.state = ExtState::Disabled;
            } else {
                ext.state = ExtState::Normal;
            }
            let hash = ext.loaded.hash.clone();
            let handles = ext.loaded.manifest.handles.clone();
            (hash, handles)
        };
        if disabled {
            self.nameplates.lock().expect("nameplates").remove_extension(name);
            self.diagnostics.lock().expect("diag").record(
                name,
                "disable",
                "disabled",
                "已禁用",
                now_ms(),
            );
        }
        if let Some(audit) = &self.audit {
            let mut audit = audit.lock().expect("audit");
            if let Ok(commit) = audit.commit_event(
                if disabled {
                    AuditEvent::Disable
                } else {
                    AuditEvent::Install
                },
                name,
                if disabled { "disabled" } else { "normal" },
                &hash,
                &handles,
                None,
            ) {
                self.push_state(
                    name,
                    if disabled {
                        AuditEvent::Disable
                    } else {
                        AuditEvent::Install
                    },
                    if disabled {
                        ExtState::Disabled
                    } else {
                        ExtState::Normal
                    },
                    commit,
                );
            }
        }
        Ok(())
    }

    /// 卸载。
    pub fn uninstall(&self, name: &str) -> ExtResult<()> {
        let Some(arc) = self.exts.lock().expect("exts").remove(name) else {
            return Err(ExtError::Spawn(format!("unknown extension {name}")));
        };
        self.order.lock().expect("order").retain(|item| item != name);
        self.nameplates.lock().expect("nameplates").remove_extension(name);
        let (hash, handles) = {
            let mut ext = arc.lock().expect("ext");
            if let Some(process) = ext.process.take() {
                process.kill();
            }
            (ext.loaded.hash.clone(), ext.loaded.manifest.handles.clone())
        };
        if let Some(audit) = &self.audit {
            let mut audit = audit.lock().expect("audit");
            if let Ok(commit) = audit.commit_event(
                AuditEvent::Uninstall,
                name,
                "uninstalled",
                &hash,
                &handles,
                None,
            ) {
                self.push_state(name, AuditEvent::Uninstall, ExtState::Normal, commit);
            }
        }
        Ok(())
    }

    /// **唯一**的重载入口（EX-37 / EX-38）：watch / git HEAD / 显式命令都打这里。
    ///
    /// 重载是**扩展级**的：一次只影响一个扩展；失败时旧的继续服务（EX-41）。
    pub fn reload(&self, name: &str) -> ExtResult<()> {
        let Some(arc) = self.extension(name) else {
            return Err(ExtError::Spawn(format!("unknown extension {name}")));
        };
        let dir = arc.lock().expect("ext").loaded.dir.clone();
        let loaded = LoadedManifest::load(&dir).map_err(|error| {
            self.diagnostics
                .lock()
                .expect("diag")
                .record(name, "reload", "error", &error.to_string(), now_ms());
            ExtError::ReloadFailed(error.to_string())
        })?;

        let artifact = manifest::resolve_artifact(&self.config, &loaded.manifest)?;
        let delivery = handle::deliver(
            &loaded.manifest.handles,
            &self.config.handles,
            self.config.limits.handle_bytes,
        )
        .map_err(ExtError::MissingHandle)?;
        // 新内容解析失败 → 旧的继续服务 + 一条诊断（EX-44）。
        let problems = handle::validate(&delivery.handles);
        if !problems.is_empty() {
            self.diagnostics.lock().expect("diag").record(
                name,
                "reload",
                "error",
                &problems.join("; "),
                now_ms(),
            );
            return Err(ExtError::ReloadFailed(problems.join("; ")));
        }
        let process = self
            .spawn_one(&loaded, &artifact, &delivery.handles)
            .map_err(|error| {
                self.diagnostics
                    .lock()
                    .expect("diag")
                    .record(name, "reload", "error", &error.to_string(), now_ms());
                ExtError::ReloadFailed(error.to_string())
            })?;
        let names = match process.handshake(
            PROTOCOL,
            &loaded.hash,
            &delivery.handles,
            Duration::from_millis(self.config.limits.handshake_timeout_ms),
        ) {
            Ok(names) => names,
            Err(error) => {
                process.kill();
                self.diagnostics
                    .lock()
                    .expect("diag")
                    .record(name, "reload", "error", &error.to_string(), now_ms());
                return Err(ExtError::ReloadFailed(error.to_string()));
            }
        };
        // 名牌冲突同样拒载；旧进程继续服务。
        if let Err(conflict) = self
            .nameplates
            .lock()
            .expect("nameplates")
            .install(name, &names, false)
        {
            process.kill();
            return Err(ExtError::NameplateConflict {
                name: conflict.name,
                winner: conflict.winner,
                loser: conflict.loser,
            });
        }

        let mut ext = arc.lock().expect("ext");
        if let Some(old) = ext.process.replace(process) {
            // 在飞调用仍握着旧的 Arc（call id 路由），这里只把新调用指过去。
            if old.is_alive() {
                // 旧进程在飞调用收完后自己 drop；这里不强杀。
            }
        }
        // 老进程贡献的名字清掉，再装新的。
        let old_names = ext.nameplates.clone();
        for nameplate in &old_names {
            if !names.contains(nameplate) {
                self.nameplates
                    .lock()
                    .expect("nameplates")
                    .remove_extension(name);
            }
        }
        for nameplate in &names {
            let _ = self
                .nameplates
                .lock()
                .expect("nameplates")
                .install(name, std::slice::from_ref(nameplate), false);
        }
        ext.loaded = loaded;
        ext.artifact = artifact;
        ext.content = delivery.handles;
        ext.nameplates = names;
        ext.state = ExtState::Normal;
        ext.crash_count = 0;
        ext.version = ext.loaded.manifest.version.clone();
        let hash = ext.loaded.hash.clone();
        let handles = ext.loaded.manifest.handles.clone();
        drop(ext);

        self.diagnostics.lock().expect("diag").clear();
        {
            let mut reloads = self.reloads.lock().expect("reloads");
            *reloads.entry(name.to_string()).or_insert(0) += 1;
        }
        if let Some(audit) = &self.audit {
            let mut audit = audit.lock().expect("audit");
            if let Ok(commit) = audit.commit_event(
                AuditEvent::Install,
                name,
                "normal",
                &hash,
                &handles,
                None,
            ) {
                self.push_state(name, AuditEvent::Install, ExtState::Normal, commit);
            }
        }
        Ok(())
    }

    pub fn count_reloads(&self, name: &str) -> u32 {
        self.reloads
            .lock()
            .expect("reloads")
            .get(name)
            .copied()
            .unwrap_or(0)
    }

    /// watch 触发源：看到目录变化就登记；**窗口内合并**，窗口后只重载一次。
    pub fn watch_tick(&self) -> Vec<String> {
        let now = now_ms();
        let names = self.names();
        let mut changed = Vec::new();
        {
            let mut signatures = self.signatures.lock().expect("signatures");
            for name in &names {
                let Some(arc) = self.extension(name) else {
                    continue;
                };
                let dir = arc.lock().expect("ext").loaded.dir.clone();
                let signature = dir_signature(&dir);
                let previous = signatures.insert(name.clone(), signature.clone());
                if previous.as_deref() != Some(signature.as_str()) {
                    changed.push(name.clone());
                }
            }
        }
        {
            let mut pending = self.pending.lock().expect("pending");
            for name in changed {
                pending.entry(name).or_insert(now);
            }
        }
        let debounce = self.config.limits.reload_debounce_ms;
        let due: Vec<String> = {
            let pending = self.pending.lock().expect("pending");
            pending
                .iter()
                .filter(|(_, since)| now.saturating_sub(**since) >= debounce)
                .map(|(name, _)| name.clone())
                .collect()
        };
        let mut reloaded = Vec::new();
        for name in due {
            self.pending.lock().expect("pending").remove(&name);
            if self.reload(&name).is_ok() {
                reloaded.push(name);
            }
        }
        reloaded
    }

    /// 显式命令触发源。与 watch / git HEAD 打的是**同一个**入口。
    pub fn explicit_reload(&self, name: &str) -> ExtResult<()> {
        self.reload(name)
    }

    /// git HEAD 触发源：HEAD 变了就重载全部（同一入口）。
    pub fn git_head_tick(&self) -> Vec<String> {
        let head = std::fs::read_to_string(self.config.exts_root.join("../.git/HEAD"))
            .unwrap_or_default();
        let mut seen = self
            .signatures
            .lock()
            .expect("signatures")
            .get("<git>")
            .cloned();
        if seen.as_deref() == Some(head.as_str()) {
            return Vec::new();
        }
        self.signatures
            .lock()
            .expect("signatures")
            .insert("<git>".to_string(), head.clone());
        seen = None;
        let _ = seen;
        self.names()
            .into_iter()
            .filter(|name| self.reload(name).is_ok())
            .collect()
    }

    /// 会话选中的名牌，提供它的扩展被禁用 / 没了 → 明确失败并指名（EX-76）。
    pub fn check_selection(&self, selected: &[String]) -> ExtResult<()> {
        let missing = self
            .nameplates
            .lock()
            .expect("nameplates")
            .missing(selected);
        if missing.is_empty() {
            return Ok(());
        }
        for name in &missing {
            let disabled = self.names().into_iter().find(|ext| {
                self.state_of(ext) == Some(ExtState::Disabled)
                    && self
                        .extension(ext)
                        .and_then(|arc| arc.lock().ok().map(|ext| ext.nameplates.clone()))
                        .map(|names| names.contains(name))
                        .unwrap_or(false)
            });
            if let Some(ext) = disabled {
                return Err(ExtError::HardLimit {
                    class: FailureClass::Error,
                    detail: format!("error=会话选了 {name}，提供它的扩展已被禁用"),
                });
            }
        }
        Err(ExtError::HardLimit {
            class: FailureClass::Error,
            detail: format!("error=会话选了 {}，提供它的扩展不在了", missing.join(",")),
        })
    }

    /// 写系统提示词：内容没变 = 零增量（EX-14）。
    pub fn ensure_system(&self, conversation: i64, generation: i64, text: &str) -> Option<(i64, bool)> {
        let audit = self.audit.as_ref()?;
        let mut audit = audit.lock().expect("audit");
        audit.ensure_system(conversation, generation, text).ok()
    }

    pub fn audit(&self) -> Option<std::sync::MutexGuard<'_, Audit>> {
        self.audit.as_ref().map(|audit| audit.lock().expect("audit"))
    }

    /// 审计库的共享句柄（场景要铺底时用）。
    pub fn audit_arc(&self) -> Option<Arc<Mutex<Audit>>> {
        self.audit.clone()
    }

    /// 某个扩展的（计数, 段, 错误类）列表。
    pub fn diagnostics_for(&self, name: &str) -> Vec<(u32, String, String)> {
        self.diagnostics
            .lock()
            .expect("diag")
            .counts_for(name)
    }

    /// 某个扩展进程的退码。
    pub fn exit_code_of(&self, name: &str) -> Option<i32> {
        self.extension(name).and_then(|arc| {
            arc.lock()
                .ok()
                .and_then(|ext| ext.process.as_ref().and_then(|process| *process.flags.exit_code.lock().ok()?))
        })
    }

    pub fn diagnostics_snapshot(&self) -> (u32, usize) {
        let diagnostics = self.diagnostics.lock().expect("diag");
        let total: u32 = self
            .names()
            .iter()
            .map(|name| diagnostics.memory_count(name))
            .sum();
        (total, diagnostics.log_lines())
    }

    pub fn count_kind(&self, conversation: i64, kind: &str) -> usize {
        self.audit
            .as_ref()
            .and_then(|audit| audit.lock().expect("audit").count_kind(conversation, kind).ok())
            .unwrap_or(0)
    }

    /// 给场景用：直接跑一次工具（不登记 gate 的那个版本已在 `call_tool` 里）。
    pub fn tool_receipt(&self, name: &str) -> Option<Arc<ProcExt>> {
        self.extension(name)
            .and_then(|arc| arc.lock().ok().and_then(|ext| ext.process.clone()))
    }

    /// 本次启动的 id（重启后不同）。
    pub fn boot_id(&self) -> u64 {
        self.boot_id
    }

    /// 进程死没死。
    pub fn crashed_of(&self, name: &str) -> bool {
        self.extension(name)
            .and_then(|arc| {
                arc.lock()
                    .ok()
                    .and_then(|ext| ext.process.as_ref().map(|process| process.flags.crashed.load(Ordering::SeqCst)))
            })
            .unwrap_or(true)
    }

    /// 直接杀掉一个扩展的进程（名牌随之消失，EX-23）。
    pub fn kill_process(&self, name: &str) {
        if let Some(arc) = self.extension(name) {
            let mut ext = arc.lock().expect("ext");
            if let Some(process) = ext.process.take() {
                process.kill();
            }
            ext.state = ExtState::Degraded;
        }
        self.nameplates.lock().expect("nameplates").remove_extension(name);
    }

    /// 这个扩展崩过几次（内存计数，重启归零）。
    pub fn crash_count(&self, name: &str) -> u32 {
        self.extension(name)
            .and_then(|arc| arc.lock().ok().map(|ext| ext.crash_count))
            .unwrap_or(0)
    }

    /// 一个扩展的进程 pid（判据读 /proc 用）。
    pub fn pid_of(&self, name: &str) -> Option<u32> {
        self.extension(name)
            .and_then(|arc| arc.lock().ok().and_then(|ext| ext.process.as_ref().map(|p| p.pid)))
    }

    /// 一个扩展的 uid（判据读它两两不同）。
    pub fn uid_of(&self, name: &str) -> Option<u32> {
        self.extension(name).and_then(|arc| arc.lock().ok().and_then(|ext| ext.uid))
    }

    /// 扩展的目录（canary / cwd 检查用）。
    pub fn dir_of(&self, name: &str) -> Option<PathBuf> {
        self.extension(name).and_then(|arc| arc.lock().ok().map(|ext| ext.loaded.dir.clone()))
    }

    /// 清单（判据读 sections / handles / version）。
    pub fn manifest_of(&self, name: &str) -> Option<Manifest> {
        self.extension(name).and_then(|arc| arc.lock().ok().map(|ext| ext.loaded.manifest.clone()))
    }

    /// 上一次写的系统提示词（场景读回）。
    pub fn system_text(&self, conversation: i64) -> Option<String> {
        let audit = self.audit.as_ref()?;
        let audit = audit.lock().expect("audit");
        let entries = audit.entries_of(conversation).ok()?;
        entries
            .iter()
            .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("pi.system"))
            .next_back()
            .and_then(|entry| entry.get("data")?.get("text")?.as_str().map(str::to_string))
    }
}

/// 目录签名：排序后的 (相对路径, 大小, mtime)。够判"变了没有"，且确定性。
pub fn dir_signature(dir: &std::path::Path) -> String {
    let mut rows: Vec<String> = Vec::new();
    walk_signature(dir, dir, &mut rows);
    rows.sort();
    crate::sha256::hex(rows.join("\n").as_bytes())
}

fn walk_signature(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|entry| entry.ok().map(|item| item.path())).collect();
    paths.sort();
    for path in paths {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let relative = path
            .strip_prefix(root)
            .map(|rel| rel.to_string_lossy().to_string())
            .unwrap_or_default();
        if metadata.is_dir() {
            walk_signature(root, &path, out);
        } else {
            let mtime = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            out.push(format!("{relative}:{}:{mtime}", metadata.len()));
        }
    }
}

/// 墙上时钟（毫秒）。**内核层零时钟**；这是应用层，可以读。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
