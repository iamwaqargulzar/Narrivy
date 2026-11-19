# Privacy Policy Engine

Policy is evaluated independently of UI and plugins.

Inputs:
- active app/window/domain category where available;
- Privacy Zone match;
- Mode/Profile policy;
- requested context fields;
- processing destination (local process, LAN compute, remote provider);
- history retention setting;
- plugin permissions.

Outputs:
- allow/deny;
- required redactions;
- retention duration;
- user confirmation requirement;
- reason code suitable for UI.

Precedence:

`Privacy Zone > explicit user one-shot decision > organization policy > Mode/Profile > global default > plugin preference`.
