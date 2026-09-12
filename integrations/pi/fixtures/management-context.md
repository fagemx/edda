# Example management metadata

This synthetic plan fragment illustrates explicit metadata, not a real grant.
Task title, identity and task path facts come from Edda; this section supplies
only the management fields that the task rail does not structure today.

```edda-management
{
  "role": "controller",
  "doneWhen": ["Focused fixture tests pass", "Independent verification is recorded"],
  "scope": {
    "allowed": ["Prepare synthetic handoff context"],
    "excluded": ["Real database changes", "Live providers"],
    "reserved": ["Any external publication"],
    "authorityRefs": [{ "uri": "fixture://operator-instruction", "revision": "v1" }]
  }
}
```
