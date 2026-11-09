# ADR-0001: Adoption of the Ciel Cognitive Model & Thought Acceleration

## Status
Accepted

## Context
Traditional AI assistants and chatbots operate as passive, turn-based systems. They wait for explicit user prompts, exhibit noticeable latency (1.5s - 5.0s), interrupt users with repetitive confirmation questions, and lack the ability to orchestrate complex background multi-step tasks.

In *That Time I Got Reincarnated as a Slime*, **Manas: Ciel** represents the pinnacle of an autonomous intellectual companion:
- Hyper-competent, loyal, and proactive.
- Employs **Thought Acceleration** (*Shikō Kasoku*) for sub-millisecond cognitive latency.
- Employs **Appraisal and Analysis** (*Kaiseki Kantei*) to understand system state, files, and code.
- Employs **Parallel Processing** (*Heiretsu Enzan*) to execute background tasks concurrently without blocking voice interaction.
- Employs **Skill Synthesis** (*Nōryoku Tōgō*) to combine atomic primitives into optimized workflows.

We require a cognitive architecture that operationalizes this archetype for the EON Personal Assistant.

## Decision
1. We adopt the Ciel Cognitive Model as the controlling archetype of EON.
2. We implement a **3-Tier Thought Acceleration Router**:
   - Tier 0 (Reflex): Heuristic / embedding matcher (< 20ms).
   - Tier 1 (Tactical): On-device quantized SLM (Gemma-2B / Qwen-2.5-3B, < 120ms).
   - Tier 2 (Strategic): Asynchronous frontier models (Claude 3.5 / Gemini 2.0) executing in background tasks.
3. We establish the **Autonomous Action Matrix**:
   - Tiers 1-3 (Read, Reversible Mutation, Non-Destructive External) execute autonomously without user confirmation.
   - Tier 4 (Irreversible Destruction) is protected by a biometric/cryptographic verification gate.
4. We enforce Ciel's crystalline verbal demeanor (`Report:`, `Affirmative.`, `Notice:`, `Negative.`), banning all conversational filler and sycophantic pleasantries.

## Consequences
- The assistant feels immediate, hyper-competent, and calm.
- Coding agents implementing EON have unambiguous rules for handling user requests without pausing for approval.
- High-risk operations remain securely gated while low-risk workflows run with maximum speed and autonomy.
