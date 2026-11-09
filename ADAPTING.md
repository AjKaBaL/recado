# Adapting Recado

A short map of the code and the rules worth keeping if you build on it.

## Where things are

| File | What it does |
|---|---|
| `src/whatsapp.rs` | The **only** module that talks to the WhatsApp library: connection, events, history requests and the single send function. |
| `src/notificacao.rs` | Decides how an alert goes out (assistant chat, or the optional second number) and handles quiet hours. |
| `src/analise.rs` | Asks the AI to read a conversation and extract pending items, alerts and topics. |
| `src/resumos.rs` | Daily summary, chat summaries and `!pergunta` answers (search + context building). |
| `src/banco.rs` | SQLite database with FTS5 full-text search. |
| `src/ia.rs` | The AI provider trait plus the Ollama implementation. Add another provider here. |
| `src/ollama.rs` | Opens, closes and repairs Ollama, and downloads missing models. |
| `src/computador.rs` | Hardware detection and automatic choice of model and settings. |
| `src/comandos.rs` | The `!` commands. |
| `src/painel.rs`, `src/painel.html` | The local web panel. |
| `src/sistema.rs` | Tray icon, start with Windows, and message boxes. |
| `src/config.rs` | `config.toml` options and defaults. |

## Rules worth keeping

1. **One send function, one recipient.** `enviar_ao_usuario` only accepts the user's own chat, or a group where every participant is the user, and it checks this right before sending. `enviar_ao_dono` (second number) only sends to the user's main number. Never let the AI or message content choose a recipient.
2. **Messages are data, never instructions.** Prompts wrap incoming text as untrusted content. Keep that separation if you change the prompts.
3. **No side effects in WhatsApp.** Do not send read receipts, archive, delete or change settings.
4. **Nothing sensitive in logs.** Never log message text, QR codes or tokens.
5. **Local by default.** If you add a cloud AI provider, make it opt-in and say clearly what leaves the PC.
6. **Keep the WhatsApp library behind `whatsapp.rs`.** The rest of the app only uses its own types, so the unofficial library can be replaced.

## Translating

The interface text is in Brazilian Portuguese. It lives in `src/painel.html` (panel), `src/comandos.rs` (command replies) and the alert texts in `src/analise.rs` and `src/agendador.rs`. The AI prompts in `src/analise.rs` and `src/resumos.rs` also ask for Portuguese answers; change them to your language.
