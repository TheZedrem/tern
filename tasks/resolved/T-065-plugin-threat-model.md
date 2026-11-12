---
status: resolved
blocked-by: []
---
# T-065: STRIDE threat model for plugins

Model the Tern ↔ plugin process boundary as `spec/threat-model/0005-plugins.md`
before any plugin code is written (architecture §Roadmap, constraints for
the plugin phases): a plugin spoofing another's panel or confirmation bar,
confirmation spam, text that hides what a command does, terminal output
reaching a plugin, a manifest changing under an enabled plugin, floods,
child processes left behind (spec 0016, Open questions), and what Tern
can't enforce on a plugin's own process (accepted in OQ-031, with a
trusted-source warning). Add it to the register and file residual risks as an
OQ for the owner. Area: meta.

Spec: ../spec/0016-plugin-system.md
