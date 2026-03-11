---
stepsCompleted: ['step-01-init', 'step-02-discovery', 'step-02b-vision', 'step-02c-executive-summary', 'step-03-success', 'step-04-journeys', 'step-05-domain', 'step-06-innovation', 'step-07-project-type', 'step-08-scoping', 'step-09-functional', 'step-10-nonfunctional', 'step-11-polish']
inputDocuments: ['prd-v1-archive.md', 'architecture.md', 'PIPELINE.md']
workflowType: 'prd'
classification:
  projectType: 'developer_tool + infrastructure_platform + saas'
  domain: 'ai_infrastructure / agentic_systems'
  complexity: 'very_high'
  projectContext: 'brownfield_major_rewrite'
  target: 'dev_solo → teams → enterprise + Hub SaaS'
architecture:
  transport: 'ZMQ XPUB/XSUB + Protobuf (Rust)'
  broker: 'long-running daemon (Rust core)'
  streams: 'pub/sub channels (ex-TTY) avec persistence, compaction cron, backup git'
  stream_providers: 'local | weaviate | elasticsearch | ...'
  stream_capabilities_core: 'pubsub, persistence, compaction (day/week/month), git backup'
  stream_capabilities_optional: 'historical_query, semantic_search (provider-dependent)'
  agents: 'ZMQ subscribers/publishers — provider-agnostic'
  surfaces: 'ZMQ subscribers — SDK multi-langage (Rust, Python, JS...)'
  providers: 'launchers d agents (Podman, AWS, Azure...)'
  storage: 'git backend (config + résumés compactés)'
  cli: 'Rust, binaire standalone'
---

# Product Requirements Document — wheelhouse

**Author:** Nico
**Date:** 2026-03-10

## Executive Summary

*Wheelhouse is the operating infrastructure for autonomous agent factories — specify, deploy, monitor, and let your agents operate their own infrastructure.*

Wheelhouse is a specification, deployment, and monitoring framework for multi-provider agentic infrastructure. It allows declarative definition of agent, stream, surface, and cron topologies via `.wh` files — an open standard (`apiVersion: wheelhouse.dev/v1`, JSON Schema validated) designed to be the Dockerfile of agentic infrastructure. Topologies deploy across any provider — local Podman, AWS Bedrock, Azure, or Wheelhouse Cloud — with identical semantics. Agents operate their own factory autonomously: they read, modify, and apply `.wh` files to scale, adapt, or evolve their infrastructure without human intervention. The Rust CLI distributes a zero-dependency standalone binary.

Target users range from solo developers deploying their first local agents to enterprises managing agent fleets in production, with Wheelhouse Cloud as the managed provider for scale.

### What Makes This Special

The core differentiator: **infrastructure as code where the agent is the operator**. Unlike existing orchestrators (LangGraph, CrewAI, AutoGen) — frameworks written by humans for humans — Wheelhouse is infrastructure operated by agents themselves. The `.wh` file is not static config — it is a living topology that agents modify as their needs evolve.

The stream is the central connective tissue: a real-time, multi-subscriber, language-agnostic typed object bus around which agents, surfaces, skills, and crons gravitate. The protocol is open and extensible — base types (`TextMessage`, `FileMessage`, `Reaction`, `SkillInvocation`, `CronEvent`) ship with Wheelhouse; custom surfaces register their own types (sketch objects, spreadsheet cells, molecular structures). Stream providers expose optional capabilities based on deployment — historical query by time interval and semantic vector search — enabling agents to retrieve relevant context across months of operation.

Surfaces are the bridge between human users and the stream — standard surfaces (Telegram, WhatsApp, CLI) ship with Wheelhouse; custom surfaces are built via the multi-language SDK, enabling any business UI to participate in a stream.

Skills are versioned recipes stored in git, loaded on demand, executed via stream objects without coupling to any LLM's native tool-calling format. Cron jobs are first-class infrastructure primitives with their own provider — they publish typed `CronEvent` objects into target streams on a schedule, driving compaction, agent wake-ups, or any recurring operation.

`wh deploy plan` gives agents and operators a transactional preview before any topology change is applied — the foundational safety mechanism that makes autonomous agent operation viable.

Wheelhouse exposes a unified observability interface — `wh ps`, `wh ls`, `wh logs`, `wh status` — consistent across all providers. What runs on Podman locally is inspected with the same commands as what runs on Wheelhouse Cloud.

Every infrastructure change is attributed to a specific agent identity — you always know which agent made which decision and when. Agent commits form an infrastructure journal: a tamper-evident record of every autonomous decision.

## Success Criteria

### User Success

**Onboarding (Day 1)**
- A developer goes from `brew install wheelhouse` to a first running agent in under 5 minutes — excluding third-party token setup (Telegram, WhatsApp) which is outside wheelhouse's control
- `wh deploy apply` provisions containers, initializes the git repo, and records the first commit with zero manual configuration
- A second node joins the private mesh in under 2 minutes with zero network configuration

**Steady State — Continuous Improvement Loop**
- An agent running for 30 days has autonomously modified its own `.wh` at least 20 times based on signals from the stream, without human intervention
- An agent can autonomously review and modify its own infrastructure at any time, based on any signal — stream events, compaction summaries, user messages, or its own judgment
- Human intervention reduces over time, limited to strategic decisions only

### Business Success

Absolute numbers are premature at this stage. Success is measured by qualitative signals with explicit timelines:

**By Month 6**
- `.wh` files published publicly by unknown users (not the core team)
- GitHub issues opened by strangers
- At least one independent developer has built a custom surface with the SDK

**By Month 12**
- Third-party providers implemented by the community
- Independent articles comparing wheelhouse to Terraform or Docker — even critical ones
- At least one paying customer operating agentic infrastructure on Wheelhouse Cloud in production
- At least one production agent running 30 days without human intervention

**Security posture (ongoing)**
- Wheelhouse is never associated with security concerns due to unvalidated third-party code. Skills distribution is curated and validated — not a community free-for-all.

### Technical Success

Built on distilled lessons from Claude Code, BMAD, OpenClaw, and direct agentic app development experience:

| Signal | Source lesson |
|--------|--------------|
| First agent running < 5 min | OpenClaw — zero friction onboarding |
| Skills lazy-loaded, manifest-driven | BMAD — no token stuffing |
| Agent commits its own `.wh` | Claude Code — agent as real operator |
| ZMQ replaces all filesystem IPC | Direct experience — filesystem IPC is fragile |
| `wh deploy plan` before every apply | Terraform — learned the hard way |
| Zero-dependency Rust binary | Python distribution frustration |

**Technical SLOs (MVP)**
- Stream message latency <10ms median, <50ms p99 (local provider)
- Broker stable under 100 simultaneous agents

### Measurable Outcomes

| Horizon | Outcome |
|---------|---------|
| Day 1 | First agent running < 5 min from install |
| Week 1 | First autonomous `wh deploy plan` generated by an agent |
| Month 1 | Agent has modified its own infra at least once without human intervention |
| Month 3 | Human intervenes only for strategic decisions |
| Month 6 | First unknown user publishes a `.wh` file publicly |
| Month 12 | At least one production agent running 30 days unattended on Wheelhouse Cloud |

## Product Scope & Phased Development

**Core hypothesis to validate:** *An agent autonomously modifies its own `.wh` based on signals from the stream — without human intervention.*

**MVP Approach:** Platform MVP — deliver the technical core (ZMQ broker, local stream, Podman agent, Rust CLI) with a functional surface (Telegram) and a complete end-to-end workflow. Goal: validate the fundamental architecture before scaling.

**Resource Requirements:** Solo dev (Nico) + BMAD as development partner. No team constraints.

### Phase 1 — MVP

- Rust CLI — `wh deploy apply/plan/destroy/lint`, `wh ps`, `wh ls`, `wh logs`, `wh status`, `wh restart`
- Podman provider (local)
- Stream service — pubsub + persistence + compaction (daily) + git backup
- Git backend — config versioning + agent-attributed commits
- Surfaces — CLI + Telegram
- Skills — git storage, lazy-loaded, manifest-driven, `SkillInvocation` as stream object
- Cron — local provider, `CronEvent` on streams
- SDK Python — minimal custom surface support

### Phase 2 — Growth

- `wh topology` — visual topology interface
- Mesh — Wheelhouse Cloud + Headscale, `wh mesh join`
- Additional providers — AWS Bedrock, Azure, Elasticsearch, Weaviate
- Wheelhouse Cloud — managed provider
- WhatsApp surface + JS/TS SDK
- Stream optional capabilities — historical query, semantic vector search
- Weekly/monthly compaction horizons
- Agent identity — GPG-signed commits, cryptographic agent PKI
- `apiVersion: wheelhouse.dev/v1` — open spec, JSON Schema

### Phase 3 — Vision

- Wheelhouse Cloud as a full managed agentic infrastructure platform
- Validated skill distribution — curated, signed, audited
- `.wh` as an industry-open standard for agentic infrastructure
- A wheelhouse agent running on Wheelhouse Cloud autonomously manages a 10-agent factory for 90 days without human intervention

### Risk Mitigation

**Technical Risks:**
- *ZMQ + Protobuf + Rust* : new stack for this project → BMAD as code partner, proven patterns
- *Atomic compaction* : corruption risk → git snapshot before each compaction, mandatory unit tests
- *Long-running broker* : daemon stability → `wh restart` in MVP, optional watchdog (systemd/launchd)

**Market Risks:**
- *Adoption* : agentic dev community still emerging → X/Reddit/YouTube + concrete use cases + installation ease as #1 criterion
- *Security trust* : no unvalidated skills → mandatory signature validation, broker localhost-only by default

**Resource Risks:**
- *Solo dev* : BMAD as productivity multiplier → rapid iterations on core MVP
- *Scope creep* : mesh, cost estimation, marketplace → Phase 2/3 only (Occam's Razor applied)

## Out of Scope (MVP)

Explicitly excluded to prevent scope creep — Growth or Vision phase items only:

- **Docker** — Podman only (Apache 2.0 license); Docker support not planned
- **Mesh networking** — Wheelhouse Cloud + Headscale is Phase 2; no multi-node in MVP
- **Cloud agent providers** — AWS Bedrock, Azure: Phase 2; Podman local only in MVP
- **Cost estimation** — `wh deploy plan` shows topology diff only; billing integration is Phase 2
- **Multi-tenancy** — single workspace per installation in MVP
- **Skills hub / marketplace** — no public skill registry; curated distribution is Phase 3
- **GPG-signed commits** — agent identity by name in commit message only in MVP; PKI is Phase 2
- **WhatsApp surface** — Telegram only in MVP; WhatsApp requires Business API approval process
- **`wh topology`** — visual topology interface is Phase 2
- **Historical query / semantic search** — optional stream provider capabilities, Phase 2
- **JS/TS and Rust SDKs** — Python SDK only in MVP

## User Journeys

### Journey 1 — Alex, Solo Dev Indie Hacker

**Opening Scene**

Alex tombe sur un thread Reddit un dimanche soir : *"Finally, something that makes agents actually deployable."* Un lien vers wheelhouse. Il clique, lit le README en 3 minutes, `brew install wheelhouse`. Il est 22h, il a deux heures devant lui.

**Rising Action**

Le vrai obstacle arrive immédiatement : ses API keys. Claude, Telegram, git credentials, Google Workspace. Wheelhouse lui propose `wh secrets init` — un wizard interactif qui détecte les providers disponibles, guide la saisie, et stocke les secrets hors git automatiquement. Cinq minutes plus tard ses credentials sont en place.

Il écrit son premier `.wh` — 15 lignes. Un agent, un stream, une surface Telegram. `wh deploy plan` lui montre exactement ce qui va se passer avant d'appliquer. `wh deploy apply`. Son agent répond dans Telegram en moins de 2 minutes.

**Climax**

Le lendemain matin, Alex reçoit un message Telegram de son agent : *"I noticed you have 3 recurring meetings this week. I drafted a skill to auto-summarize them. Shall I commit it?"* L'agent a détecté un pattern dans le stream, écrit un skill, et demande validation. Alex répond *"yes"*. Le skill est committé, appliqué. Aucune ligne de code écrite par Alex.

**Resolution**

Une semaine plus tard, Alex pousse son `.wh` sur GitHub public. Trois inconnus l'ont forké. Il n'a toujours pas rouvert son éditeur de code.

**Journey Requirements Revealed**
- `wh secrets init` — wizard de setup des credentials, secrets hors git
- `wh deploy plan` — preview obligatoire avant apply
- Onboarding < 5 min hors API keys
- Agent qui propose des modifications de son propre `.wh`
- Telegram comme premier canal de validation humain

---

### Journey 2 — Sarah, Platform Engineer Enterprise

**Opening Scene**

Sarah lit un article Medium un mardi matin : *"We replaced our entire agent infrastructure with 40 lines of YAML."* Elle est sceptique. Mais l'angle GitOps l'accroche. Elle forward l'article au CTO avec un seul mot : *"Interesting?"*. Le CTO répond en 3 minutes : *"POC. 2 weeks."*

**Rising Action**

Premier réflexe : sécurité et souveraineté des données. Avant même d'installer, elle lit la doc. Deux questions critiques : où tourne le compute, où vivent les données ? La réponse est claire — tout est self-hosted par défaut, Podman local ou leur propre cloud provider. Aucune donnée ne quitte leur infrastructure sans configuration explicite.

Elle installe wheelhouse sur un node de staging. Elle migre un de leurs 6 agents existants en 45 minutes. Son `.wh` fait 22 lignes. `wh deploy plan` lui montre exactement les ressources créées avant d'appliquer.

**Climax**

Jour 8 du POC. Sarah a migré 4 agents. `wh ps` lui donne une vue unifiée — état, provider, stream actif, dernier commit. Elle clique sur l'historique git d'un agent : chaque modification est attribuée, horodatée, réversible. Elle pense à l'incident de la semaine dernière — avec wheelhouse, elle aurait su en 30 secondes quel agent avait modifié quoi et pourquoi.

**Resolution**

Le lundi suivant, Sarah présente au CTO. 4 agents migrés en 8 jours, zéro incident depuis la migration. Le CTO demande : *"Quand on migre les 2 restants ?"*

**Journey Requirements Revealed**
- Data locality explicite — aucune ambiguïté sur où tourne le compute
- `wh deploy plan` — preview des ressources avant apply
- `wh ps` — vue unifiée multi-agents avec état en temps réel
- Historique git attribué par agent — audit trail lisible
- Migration d'agents existants sans réécriture complète
- Self-hosted by default, Wheelhouse Cloud opt-in explicite

---

### Journey 3 — Donna, Agent Manager

**Opening Scene**

03:47. Le cron quotidien vient de terminer la compaction du stream. Donna lit le résumé du jour : 847 messages traités, le researcher a timeout 4 fois en fin de journée, les résumés Telegram dépassent systématiquement 400 caractères depuis lundi. Ce n'est pas une alerte. C'est une observation. Donna commence à raisonner.

**Rising Action**

Elle ouvre le git log de son `.wh`. Dernier commit : 6 jours ago, par elle-même. Ça n'a pas suffi. Elle génère un `wh deploy plan` :

```
~ agent researcher
  replicas: 1 → 2
~ skill summarize-telegram
  max_length: unlimited → 380
```

Elle vérifie le pattern de surcharge sur 7 jours. Le coût est justifié. Elle applique.

**Climax**

08:23. Alex reçoit un message Telegram de Donna : *"Scaled researcher to 2 replicas and capped Telegram summaries at 380 chars. Reason: 4 daily timeouts and UX degradation detected over 6 days. Git: a3f9c2."*

Alex tape : *"Good call."* Il ne touche à rien.

**Resolution**

30 jours plus tard, Donna a fait 23 commits autonomes. Alex a validé manuellement 2 fois — les deux fois pour des décisions qui dépassaient son seuil de tolérance configuré. Pour tout le reste, il a lu et laissé faire.

**Journey Requirements Revealed**
- Agent lit la compaction comme signal de décision parmi d'autres
- `wh deploy plan` exécutable par l'agent lui-même
- Commit message auto-généré avec justification lisible
- Notification Surface après chaque modification autonome
- Seuil de validation humaine configurable

---

### Journey 4 — Marc, SRE/Ops

**Opening Scene**

Lundi 09:15. Marc ouvre `wh ps` — son premier réflexe du matin. 6 agents, tous verts. 3 streams actifs. Café.

14:47. Alerte Telegram : *"researcher-2 : 47 iterations on same task, no output published in 23 minutes."*

Marc pose son café.

**Rising Action**

`wh logs researcher-2 --tail 100`. Pattern immédiat — l'agent boucle sur une invocation de skill qui ne répond jamais. Le skill `web-search` timeout silencieusement au lieu de publier un `SkillResult` d'erreur.

`wh deploy plan researcher-2` — il ajoute un timeout explicite sur le skill. `wh deploy apply`. L'agent redémarre, publie immédiatement un `SkillResult` d'erreur propre, et continue.

**Climax**

Marc écrit un post-mortem en 5 lignes dans le stream ops. Il ajoute un cron de détection de boucle — si un agent n'a rien publié en 15 minutes, alerte automatique.

Temps de résolution : 11 minutes.

**Resolution**

Trois semaines plus tard, Marc a commencé à construire une surface Grafana custom avec le SDK — des panels qui lisent les métriques du broker ZMQ et le git log. `wh ps` en CLI c'est bien pour le debug. Un dashboard pour la supervision continue, c'est mieux.

**Journey Requirements Revealed**
- `wh ps`, `wh logs` — observabilité CLI temps réel
- Alerting via Surface quand un agent dépasse un seuil comportemental
- `SkillResult` d'erreur obligatoire — pas de timeout silencieux
- Cron de health check configurable
- SDK Surface — Grafana comme surface custom de supervision

---

### Journey 5 — Karim, Dev Surface Custom

**Opening Scene**

Karim a 3 jours pour livrer un prototype de surface molecular sketcher. Il ouvre la doc wheelhouse, section SDK. Deux choses le rassurent : des exemples Python complets, et un MCP wheelhouse disponible pour Claude Code. Il installe le MCP en 2 minutes. Son agent de dev a maintenant accès à toute la doc SDK, les schémas Protobuf, et les patterns recommandés.

**Rising Action**

Claude Code génère un squelette complet — connexion ZMQ, enregistrement du type `MoleculeObject`, handlers publish/subscribe. Karim lit le code. Il comprend la structure sans avoir lu la spec Protobuf en entier.

```python
@wheelhouse.register_type("biotech.MoleculeObject")
class MoleculeObject(BaseStreamObject):
    smiles: str
    name: str
    metadata: dict
```

**Climax**

Jour 2, 16h. Karim envoie sa première `MoleculeObject`. L'agent analyse la structure, publie un `AnalysisResult`. Le sketcher affiche la réponse en overlay sur la molécule dessinée.

La chercheuse principale voit la démo live. Elle dit : *"C'est exactement ce qu'on voulait."*

**Resolution**

Karim pousse le code. Le `.wh` de l'équipe a été mis à jour par Donna pour inclure la nouvelle surface — Karim n'a pas eu à toucher à l'infra.

**Journey Requirements Revealed**
- MCP wheelhouse — doc SDK + schémas Protobuf accessibles aux agents de dev
- SDK Python avec exemples complets
- API d'enregistrement de types custom simple (`@wheelhouse.register_type`)
- Type namespace pour éviter les collisions (`biotech.MoleculeObject`)
- Surface hot-pluggable — `.wh` mis à jour sans redémarrer les agents existants

---

### Journey 6 — Sophie, Chercheuse Senior Biotech (End User Métier)

**Opening Scene**

Sophie a 15 ans d'expérience en chimie computationnelle. Elle jongle depuis toujours entre ChemDraw, des scripts Python qu'elle ne comprend pas, et des emails à l'IT pour lancer des simulations. Chaque itération prend des heures, parfois des jours.

Karim lui montre le molecular sketcher en 5 minutes. Elle est polie mais sceptique.

**Rising Action**

Sophie ouvre le sketcher. L'interface ressemble à ChemDraw. Elle dessine une molécule qu'elle connaît bien. Elle tape : *"What's the predicted binding affinity for this compound against EGFR?"*

4 secondes.

L'agent répond avec une valeur, une source, et trois variantes structurelles. En dessous : *"Run full docking simulation?"* Elle clique. L'agent lance en arrière-plan, la préviendra dans Telegram.

Elle continue son autre travail.

**Climax**

47 minutes plus tard, son téléphone vibre. Résultats de simulation, résumé en langage naturel, tableau comparatif, recommandation. Elle transfère à sa directrice : *"Got this in under an hour. Used to take 2 days."*

**Resolution**

Sophie utilise le sketcher 4 à 5 fois par jour. Elle ne sait toujours pas ce qu'est un stream. Elle s'en fiche. Elle dit à une collègue : *"C'est ChatGPT pour la chimie — il connaît mes outils, il les lance pour moi, il m'explique les résultats."*

Wheelhouse est invisible. C'est exactement comme ça devrait être. Sophie's infrastructure is managed by Donna autonomously — she never touches a `.wh` file.

**Journey Requirements Revealed**
- Surface métier = interface familière, pas de terminologie infra
- Chat en langage naturel intégré dans la surface
- Notification async pour les tâches longues (Telegram)
- `SkillInvocation` déclenché par un clic UI
- Wheelhouse invisible pour l'end user
- Zéro exposition à l'infrastructure sous-jacente

---

### Journey Requirements Summary

| Capability | Revealed by |
|-----------|-------------|
| `wh secrets init` wizard | Alex |
| `wh deploy plan` — preview before apply | Alex, Sarah, Donna |
| `wh ps`, `wh logs` CLI observability | Marc, Sarah |
| Agent notifies via Surface after autonomous action | Donna, Marc |
| Configurable human validation threshold | Donna |
| Behavioral alerting (agent loop detection) | Marc |
| `SkillResult` error contract — no silent timeouts | Marc |
| MCP wheelhouse for dev agents | Karim |
| SDK Python + custom Protobuf types | Karim |
| Hot-pluggable surface | Karim |
| Natural language chat in surface | Sophie |
| Async notification for long tasks | Sophie |
| Wheelhouse invisible to end user | Sophie |
| Self-hosted by default, Cloud opt-in | Sarah |
| Git history attributed by agent | Sarah, Donna |

## Domain-Specific Requirements

For an agent to operate its own infrastructure autonomously and safely, the following domain constraints apply — they are non-negotiable and must be reflected in all downstream architecture and implementation decisions.

### Compliance & Regulatory

- **GDPR by design** — stream persistence must support data deletion (right to erasure); personal data in streams must be identifiable and purgeable
- **Data residency** — provider determines where data lives; explicitly documented per provider in official docs; no ambiguity
- **SOC2** — target for Wheelhouse Cloud (Growth); self-hosted deployments are customer responsibility

### Security

- **Secrets management** — `wh secrets init` stores credentials outside git in OS keychain or encrypted local file; never in `.wh` files or git history
- **Git signature validation** — `.wh` modifications signed by agent identity; validation is mandatory and non-bypassable before any apply
- **Broker network isolation** — broker binds on localhost only by default, never `0.0.0.0`; mesh WireGuard isolates multi-node deployments
- **Tenant isolation** — on Wheelhouse Cloud, streams and agents are isolated per workspace
- **Agent keystore** — GPG key storage design must be completed before implementing agent identity in Growth; keys must not live in container filesystem

### Technical Constraints

- **LLM provider data policies** — documented per provider; what transits via Bedrock/Azure is subject to their ToS
- **Compaction atomicity** — compaction is atomic with rollback on LLM timeout; partial/corrupted compaction state is impossible
- **Stream snapshot before compaction** — backup of stream state before any compaction operation (Growth)

### Guardrails

- **`max_replicas`** — mandatory field per agent in `.wh`; prevents unconstrained autonomous scaling
- **Rate limit on autonomous apply** — max N apply operations per hour per agent (Growth)
- **Anomaly detection on deploy plan** — detect aberrant plans (e.g. destroy all agents) before autonomous apply (Growth)

### Operational Requirements

- **Cron failure alerting** — any cron failure triggers immediate Surface notification
- **`SkillResult` error contract** — skills must always publish a `SkillResult` on completion or failure; silent timeouts are forbidden
- **Agent loop detection** — an agent with no stream output for N minutes triggers Surface alert (configurable)

### Known Risks — Not Addressed in MVP

- **Prompt injection via stream** — malicious stream message manipulating agent behavior; backlog
- **Agent self-written malicious skill** — compromised agent writing a backdoor skill to git repo; backlog (partially mitigated by signed commits)
- **Provider billing explosion** — cost alerting via provider webhooks; Growth

## Innovation & Novel Patterns

### Detected Innovation Areas

**1. First IaC natively readable and writable by a LLM**

The agent-as-operator pattern is not new in concept — any process that reads state, makes decisions, and produces actions is an operator. What is genuinely new: the `.wh` format is declarative YAML that fits natively in an LLM's context window. Terraform HCL, Helm charts, Kubernetes manifests — none are designed to be written by a model. Wheelhouse is.

**2. First versioned communication contracts in the agentic world**

Existing agent frameworks communicate via natural language — JSON embedded in markdown, parsed by prompt. This is fragile, uncontracted, and unversioned. Wheelhouse introduces a typed object bus where every message has a machine-readable, versioned schema (Protobuf). An agent can verify it supports a type before subscribing. This brings API contract discipline to agentic communication.

**3. First IaC with a fully autonomous and audited observation→decision→action loop**

All existing IaC assumes human intent. The human writes desire, the machine executes it. Wheelhouse closes the loop without human intervention: the stream *is* the observation, compaction *is* the analysis, the `.wh` *is* the intent, apply *is* the action — and every step is signed and audited in git.

### Market Context & Competitive Landscape

- **Orchestrators** (LangGraph, CrewAI, AutoGen) — code frameworks for humans, not infrastructure for agents
- **IaC tools** (Terraform, Pulumi) — human-authored, not LLM-native
- **Agent platforms** (AWS Bedrock Agents, Azure AI Studio) — provider-locked, no open standard
- **OpenClaw** — personal agent assistant, not an infrastructure framework; no multi-provider, no typed bus

No existing tool combines: LLM-native IaC + typed object bus + autonomous self-modifying topology.

### Validation Approach

| Innovation | Validated when |
|-----------|---------------|
| LLM-native IaC | Agent makes its first autonomous `.wh` commit (Week 1) |
| Typed object bus | Custom surface publishes a non-core type and an agent processes it (Karim's journey) |
| Autonomous loop | Compaction → decision → apply cycle runs 30 days without human intervention (Month 1) |

### Risk Mitigation

| Innovation | Risk | Fallback |
|-----------|------|---------|
| LLM-native IaC | Agent generates invalid `.wh` | Schema validation + `wh deploy plan` catches errors before apply |
| Typed object bus | Adoption friction for custom types | JSON in `TextMessage` as transition path |
| Autonomous loop | Agent takes destructive autonomous action | `max_replicas` guardrail + configurable human validation threshold |

## CLI, SDK & Developer Experience

The CLI is wheelhouse's primary control plane — used by human operators for setup and inspection, and by agents for autonomous `wh deploy apply/plan` operations. The SDK enables surface and skill developers to participate in the stream ecosystem without understanding broker internals.

### SDK

**Language support (priority order)**
1. **Python** — first SDK; targets agent developers and scripting workflows
2. **JS/TS** — second SDK (Phase 2); targets web surfaces and frontend developers
3. **Rust** — third SDK (Phase 2); targets high-performance surfaces and core contributors

**Package managers**
- Python: `pip` / `uv`
- JS/TS: `npm` / `bun`
- Rust: `cargo`

**SDK surface** — exposes: ZMQ connection, publish/subscribe, Protobuf custom type registration, stream lifecycle management

### CLI

**Scriptability**
- All commands are scriptable — designed for CI/CD, shell scripts, and agent invocation
- `--format json` available on all commands producing structured output
- Semantic exit codes — `0` success, `1` error, `2` plan change detected (for `wh deploy plan` in scripts)

**Shell completion**
- Bash, Zsh, Fish — generated via `wh completion <shell>`

**Output design**
- Human-readable by default (colors, tables)
- `--format json` for programmatic parsing
- `--quiet` to suppress non-essential output in scripts

### Installation

- **macOS**: `brew install wheelhouse`
- **Linux**: `curl | sh` + `.deb` / `.rpm` packages
- **CI/CD**: Official GitHub Action `wheelhouse/setup-action`

### Versioning Strategy

Semver applies uniformly: `.wh` format version, Protobuf type versions, and broker API version all follow `MAJOR.MINOR.PATCH`. Minor version bumps must be backwards-compatible (FR52, NFR-E1). Breaking changes require a major version bump and a documented migration path.

### Migration

No formal migration — Nico is the sole v1 user. Only Donna's persona `.md` files are preserved — already guaranteed by `PersonaActionHandler.delete()` no-op. Everything else is rewritten from scratch.

### Documentation

- Getting started guide < 5 min (mirrors the onboarding success criteria)
- MCP wheelhouse for Claude Code / Cursor — SDK docs + Protobuf schemas + best practices
- Complete surface examples (CLI, Telegram, custom) in official docs

---

## Functional Requirements

> **Actors**: *Operator* = human or agent managing the infrastructure ; *Developer* = surface or skill creator ; *End User* = user interacting via a surface.
>
> **Scope tags**: `[MVP]` = Phase 1 ; `[P2]` = Phase 2 Growth

### Infrastructure Deployment & Lifecycle

- FR1 `[MVP]` : An operator can declare a topology of agents, streams and surfaces in a `.wh` file with format `apiVersion: wheelhouse.dev/v1`
- FR2 `[MVP]` : An operator can apply a topology with `wh deploy apply` and preview changes before application (`wh deploy plan`)
- FR3 `[MVP]` : An operator can destroy a deployed topology with `wh deploy destroy`
- FR4 `[MVP]` : An operator can define guardrails in the `.wh` (max_replicas, budget max) that block deployment if exceeded
- FR5 `[MVP]` : An agent can read, modify and apply its own `.wh` file autonomously
- FR49 `[MVP]` : A developer can install wheelhouse via a single command and obtain an operational broker
- FR57 `[MVP]` : An operator can validate the syntax of a `.wh` file without applying it (`wh deploy lint`)

### Stream Management

- FR6 `[MVP]` : An operator can create, list and delete streams via CLI
- FR7 `[MVP]` : A publisher can send typed Protobuf objects into a stream
- FR8 `[MVP]` : A subscriber can receive objects in real time from a stream
- FR9 `[MVP]` : The system persists stream objects according to provider configuration
- FR10 `[MVP]` : An operator can declare compaction rules on a stream via cron jobs (FR36), with configurable granularities (day, week, month)
- FR11 `[MVP]` : The system produces git summaries of compactions as versioned backup
- FR12 `[MVP]` : An operator can choose a stream provider (local by default)
- FR39 `[MVP]` : An operator can observe stream objects in real time (`wh stream tail`)
- FR43 `[MVP]` : An operator can configure object retention in a stream (duration or max size)
- FR44 `[MVP]` : An operator can view system health and basic broker metrics (`wh status` — connected subscribers, objects/sec)
- FR51 `[MVP]` : A subscriber can automatically reconnect to the broker after an interruption
- FR53 `[P2]` : The system guarantees configurable delivery semantics (at-least-once by default)
- FR54 `[MVP]` : The system guarantees delivery order of objects published by the same publisher in a stream
- FR56 `[P2]` : An operator can replay the N last objects of a stream locally (local replay)

### Agent Lifecycle & Autonomy

- FR13 `[MVP]` : An operator can deploy an agent via a provider (Podman local in MVP)
- FR14 `[MVP]` : An agent can subscribe to one or more streams and receive objects
- FR15 `[MVP]` : An agent can publish objects into a stream
- FR16 `[MVP]` : An agent can execute an autonomous observe → decide → act cycle
- FR17 `[MVP]` : An agent can revise its infrastructure at any time based on any signal received in a stream
- FR18 `[MVP]` : An agent identifies its actions by name in git commits
- FR41 `[MVP]` : An agent can publish an error object into a stream following a skill failure

### Skills

- FR19 `[MVP]` : A developer can create a skill as a set of markdown files + steps versioned in git
- FR20 `[MVP]` : An agent can invoke a skill via a `SkillInvocation` object published in a stream
- FR21 `[MVP]` : The system loads skills on demand (lazy loading) from the git repository
- FR22 `[MVP]` : An operator can declare the skills available to an agent in its `.wh`
- FR46 `[P2]` : A skill can declare execution dependencies (tools, libs) that the provider satisfies before invocation
- FR52 `[P2]` : An operator can read objects published by a previous minor version of wheelhouse without data loss

### Surfaces & SDK

- FR23 `[MVP]` : An end user can interact with an agent via the Telegram surface
- FR24 `[MVP]` : A developer can create a custom surface using the Python SDK
- FR25 `[MVP]` : A surface can subscribe to a stream and send/receive objects
- FR26 `[MVP]` : A surface can register custom object types in the broker type registry
- FR27 `[MVP]` : A developer can interact with an agent via the CLI surface
- FR42 `[P2]` : A developer can list object types registered in a stream (type registry discovery)
- FR45 `[P2]` : A surface can send binary objects (files, large payloads) into a stream
- FR55 `[MVP]` : A developer can test a surface or skill without a production broker (test/mock mode)

### Git Backend & Versioning

- FR28 `[MVP]` : The system versions in git the personas, TTY meta, skills, cron, users and telegram config on each deploy apply or destroy operation
- FR29 `[MVP]` : An operator can clone a git repo to restore the full infrastructure configuration on a new machine
- FR30 `[MVP]` : The system guarantees exclusion of secrets (API tokens) from git versioning
- FR50 `[P2]` : An operator can export and import a persona (SOUL.md, IDENTITY.md, MEMORY.md) via git

### CLI & Observability

- FR31 `[MVP]` : An operator can list and inspect all deployed components via `wh` CLI
- FR32 `[MVP]` : The CLI supports `--format json` for all inspection commands
- FR33 `[MVP]` : The CLI provides shell completion (bash, zsh, fish)
- FR34 `[MVP]` : An operator can consult stream messages via CLI
- FR35 `[MVP]` : The CLI displays semantic exit codes exploitable in scripts
- FR47 `[MVP]` : The system refuses connections from unauthorized processes (localhost-only by default)

### Cron & Automation

- FR36 `[MVP]` : An operator can declare cron jobs associated to streams in their `.wh`
- FR37 `[MVP]` : The system publishes a `CronEvent` object into the target stream at each trigger
- FR38 `[MVP]` : An agent can react to a `CronEvent` to execute scheduled actions

---

## Non-Functional Requirements

> *Excluded categories: Scalability (solo dev MVP, no variable traffic), Accessibility (CLI/dev tool, no public UI), Regulatory compliance (no medical/financial data).*

### Performance

- **NFR-P1 :** The system routes objects with perceptibly zero local latency (target: <10ms median, <50ms p99, measured in test)
- **NFR-P2 :** The CLI responds to any inspection command in under **500ms** (excluding network operations)
- **NFR-P3 :** Initial installation completes in under **5 minutes** on a standard machine
- **NFR-P4 :** Stream compaction does not block active subscriber read/write (git snapshot blocking window < 1 second)

### Reliability

- **NFR-R1 :** The system exposes a manual restart command (`wh restart`) ; automatic watchdog (systemd/launchd) is optional in MVP
- **NFR-R2 :** No object published in a stream is lost on restart, within the configured retention limit (FR43)
- **NFR-R3 :** Compaction is atomic — on failure, the stream is intact (git snapshot before compaction, blocking window < 1s)
- **NFR-R4 :** A subscriber automatically reconnects within **5 seconds** after an interruption

### Security

- **NFR-S1 :** The system accepts only localhost connections by default (no network exposure without explicit configuration)
- **NFR-S2 :** Secrets (API tokens, credentials) are never committed to git

### Portability & Installation

- **NFR-I1 :** wheelhouse installs on macOS ARM and Linux amd64 in MVP (no dependencies beyond Podman) ; macOS Intel + Linux arm64 in Phase 2
- **NFR-I2 :** The CLI is distributed as a standalone Rust binary (zero runtime dependency)
- **NFR-I3 :** An operator can migrate a complete infrastructure to a new machine via `git clone` + `wh deploy apply` (validated by CI test on clean machine)

### Developer Experience

- **NFR-D1 :** The Python SDK is compatible with Python 3.10+
- **NFR-D2 :** All CLI commands document their usage via `--help` without network connection
- **NFR-D3 :** System errors include a human-readable message AND a numeric code referenced in `ERRORS.md` in the repo
- **NFR-D4 :** Test/mock mode (FR55) works without Podman installed
- **NFR-D5 :** The system produces structured logs (JSON) viewable via `wh logs` with configurable level (debug/info/warn/error)

### Extensibility

- **NFR-E1 :** An operator can read objects published by a previous minor version of wheelhouse without data loss (validated by v_n → v_n+1 message fixtures)
- **NFR-E2 :** A new stream provider implements a defined Rust interface (`StreamProvider` trait) without forking the system
