# Process Sketch

{{#each phases}}
### {{name}}{{#if required}} (required){{else}} (optional){{/if}}

- **ID**: {{id}}
- **Role**: {{agent_role}}
- **Status**: {{status}}
- **Rationale**: {{rationale}}
{{#if depends_on.length}}
- **Depends on**: {{join depends_on ", "}}
{{/if}}

{{/each}}
{{#if risks.length}}
## Risks

{{#each risks}}
- {{this}}
{{/each}}

{{/if}}
> Complexity: {{complexity}} — {{complexity_rationale}}
> Persona: {{persona_hint}}
