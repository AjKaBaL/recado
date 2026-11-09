# Recado

**A private WhatsApp assistant that runs on your own Windows PC.** It reads the chats you choose,
connects topics across days, and reminds you of what matters, writing **only to you**.

🌐 English · [Português](README.pt-BR.md) · [Español](README.es.md)

> The app's interface and messages are in **Brazilian Portuguese**. See [ADAPTING.md](ADAPTING.md) to translate it.

## What it does

- **Alerts as things happen.** When someone asks you for something, sets a deadline, charges you or changes a time, Recado tells you. It judges the whole conversation, so a message that merely mentions your name is not enough.
- **Pending items and reminders.** It notes what people are waiting for, reminds you the day before a deadline, and warns you once the deadline has passed. It also understands when something got resolved later ("I'll send the quote" on Monday, the quote arrives on Thursday).
- **Questions about your history.** Write `!pergunta who promised to send me the document?` (any question) and it searches every chat and date to answer.
- **Daily summary**, a **quiet hours** window, and **local transcription of voice messages**.
- **Old history.** It can ask your phone for older messages, 50 at a time, back to the start of a chat.
- **Adjusts to your PC.** It detects your RAM, NVIDIA GPU and CPU cores, picks a suitable AI model, and downloads that model through Ollama if it's missing.
- **Optional second number.** Messages sent from your own account never make your phone ring. If you link a second WhatsApp number, alerts come from it, so your phone rings like any normal chat.
- **Minimal local panel** at `http://127.0.0.1:7878`, plus a tray icon next to the clock. There is no console window.

## Safety rules (built in)

- It **never** writes to contacts, groups or anyone else. It only writes to your own chat, or to a group where you are the **only** participant, and it checks this before every send. The AI never chooses the recipient.
- It never marks messages as read, archives, deletes, or changes WhatsApp settings.
- Incoming messages are treated as untrusted data and never as instructions (prompt-injection separation).
- Everything stays on your PC: the AI runs locally through [Ollama](https://ollama.com), and voice transcription runs locally through [whisper.cpp](https://github.com/ggml-org/whisper.cpp). Logs never contain message text, QR codes or tokens.

> ⚠️ The connection uses an **unofficial** WhatsApp Web library ([whatsapp-rust](https://crates.io/crates/whatsapp-rust)). It may break when WhatsApp changes, and there is no guarantee against account restrictions. This project is not affiliated with WhatsApp or Meta.

## Installing (Windows 10/11)

1. Install **[Ollama](https://ollama.com/download)**. Recado opens and closes it automatically and downloads the model it needs.
2. Download **`Recado.exe`** and **`instalar-ferramentas.ps1`** from [Releases](../../releases).
3. *(Optional, for voice messages)* Right-click `instalar-ferramentas.ps1` and choose **Run with PowerShell**. It downloads whisper.cpp, a speech model sized for your RAM and, if missing, FFmpeg (through winget).
4. Open `Recado.exe`. The panel opens in your browser and shows a QR code. On your phone, go to **WhatsApp → Linked devices → Link a device** and scan it.
5. In your own chat ("Message yourself"), or in a group where only you are a member (pick it first in the panel: *Configurações → Conversa do assistente*), send `!ativar`. Recado replies and from then on writes only there.

Data, the config file and logs live in `%LOCALAPPDATA%\Recado`.

### Recommended hardware

| RAM | AI model chosen | Notes |
|---|---|---|
| 16 GB+ | `qwen3` (8B) | best analysis |
| 12 GB | `qwen3:4b` | good balance |
| 8 GB | `qwen3:1.7b` | simpler, still useful |

A GPU is not required; without one, everything runs on the CPU, only slower. To pick the settings yourself, set `ajuste_automatico = false` in `config.toml`.

## Commands (in your assistant chat)

| Command | What it does |
|---|---|
| `!ajuda` | list of commands |
| `!status` | health check |
| `!pendentes` | what people are waiting for from you |
| `!feito N` / `!cancelar N` | close or discard pending item N |
| `!hoje` | summary of today |
| `!resumo NAME` | summary of one chat (last 3 days) |
| `!buscar WORDS` | search every chat and date |
| `!pergunta TEXT` | ask anything about your messages |
| `!pausar` / `!retomar` | stop / resume reading |
| `!desativar` | stop sending messages |

## Building from source

You need Rust (the `x86_64-pc-windows-gnu` toolchain) and MinGW (`windres`, which embeds the icon and the file details).

```bash
cargo build --release
cargo test
```

The binary is `target/release/recado.exe`. Self-tests that run without WhatsApp: `recado.exe --testar-ia`, `--testar-audio file.ogg`, `--testar-espaco`, and `--perguntar "question"`.

## Credits

A personal project by **AjKaBaL**, shared to help anyone who had a similar idea. Released under the [MIT License](LICENSE).
