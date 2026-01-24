Skill Ontology (Meta‑model)
A skill is a prescriptive knowledge package that encodes how to act in a domain. It has five layers:

1. Intent layer
  What the skill is for.
   - Skill
       - id, name, description
       - scope (domain, task families)
       - triggers (keywords, contexts, file patterns)
2. Capability layer
  What the skill enables you to do.
   - Capability
       - verbs (e.g., “render”, “measure”, “sequence”)
       - targets (entity types the capability applies to)
       - outcomes (expected outputs)
3. Prescription layer
  The actual rules: MUST / SHOULD / MUST_NOT.
   - Rule
       - modality (MUST / SHOULD / MUST_NOT)
       - subject (what the rule governs)
       - action (use/avoid/derive/compose)
       - condition (when it applies)
       - rationale (why)
       - severity (hard/soft constraint)
4. Procedure layer
  Repeatable workflows/pipelines.
   - Process
       - inputs / outputs
       - steps (ordered rules or patterns)
       - dependencies (packages/tools)
       - failure modes (what breaks if violated)
5. Evidence layer
  How the skill is demonstrated.
   - Pattern
       - name, description
       - anti‑pattern (what not to do)
   - Example
       - code, config, snippets
       - provenance (where it came from)

———

Minimal abstract schema

Skill:
 id: string
 description: string
 scope: [string]
 triggers: [Trigger]
 capabilities: [Capability]
 rules: [Rule]
 processes: [Process]
 patterns: [Pattern]
 examples: [Example]
 dependencies: [Dependency]

Trigger:
 match: string
 weight: number
 context: string?

Capability:
 id: string
 verbs: [string]
 targets: [string]
 outcomes: [string]

Rule:
 id: string
 modality: MUST|SHOULD|MUST_NOT
 subject: string
 action: string
 object: string
 condition: string?
 rationale: string?
 severity: hard|soft

Process:
 id: string
 inputs: [string]
 outputs: [string]
 steps: [string]        # references Rule or Pattern ids
 dependencies: [string]

Pattern:
 id: string
 description: string
 antiPattern: string?

Example:
 id: string
 language: string
 text: string
 references: [string]

Dependency:
 kind: package|api|tool
 name: string
 required: boolean
