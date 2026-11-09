# Recado

**Asistente personal y privado para WhatsApp, que funciona en tu propia PC con Windows.** Lee los chats
que elijas, conecta temas entre días y te recuerda lo importante, escribiendo **solo para ti**.

🌐 [English](README.md) · [Português](README.pt-BR.md) · Español

> La interfaz y los mensajes de la aplicación están en **portugués de Brasil**. Consulta [ADAPTING.md](ADAPTING.md) (en inglés) para traducirla.

## Qué hace

- **Avisos al momento** cuando alguien te pide algo, fija un plazo, te cobra o cambia un horario. Evalúa toda la conversación; que solo mencionen tu nombre no basta.
- **Pendientes y recordatorios.** Anota lo que esperan de ti, te recuerda el día antes del plazo y te avisa cuando el plazo ya pasó. También entiende cuando algo se resolvió después ("te mando el presupuesto" el lunes, y el presupuesto llega el jueves).
- **Preguntas sobre tu historial.** Escribe `!pergunta ¿quién quedó en mandarme el documento?`, o cualquier otra pregunta, y busca en todos los chats y fechas.
- **Resumen diario**, **horario de silencio** y **transcripción local de audios**.
- **Historial antiguo.** Pide a tu teléfono mensajes más antiguos, de 50 en 50, hasta el inicio del chat.
- **Se adapta a tu PC.** Detecta la memoria RAM, la tarjeta gráfica NVIDIA y los núcleos del procesador, elige el modelo de IA adecuado y lo descarga con Ollama si falta.
- **Segundo número (opcional).** Los mensajes enviados desde tu propia cuenta nunca hacen sonar el teléfono. Si vinculas un segundo número de WhatsApp, los avisos llegan desde él y el teléfono suena como en un chat normal.
- **Panel local sencillo** en `http://127.0.0.1:7878` e ícono junto al reloj. No queda ninguna ventana de consola abierta.

## Reglas de seguridad (incluidas)

- **Nunca** escribe a contactos, grupos ni a otras personas. Solo escribe en tu propio chat, o en un grupo donde **solo tú** participas, y lo comprueba antes de cada envío. La IA nunca elige el destinatario.
- Nunca marca como leído, no archiva, no borra y no cambia ajustes de WhatsApp.
- Los mensajes recibidos se tratan como datos, nunca como órdenes (protección contra *prompt injection*).
- Todo queda en tu PC: la IA corre localmente con [Ollama](https://ollama.com) y la transcripción con [whisper.cpp](https://github.com/ggml-org/whisper.cpp). Los registros nunca guardan el texto de los mensajes, códigos QR ni tokens.

> ⚠️ La conexión usa una biblioteca **no oficial** de WhatsApp Web ([whatsapp-rust](https://crates.io/crates/whatsapp-rust)). Puede dejar de funcionar cuando WhatsApp cambie, y no hay garantía contra restricciones en la cuenta. Este proyecto no tiene relación con WhatsApp ni con Meta.

## Instalación (Windows 10/11)

1. Instala **[Ollama](https://ollama.com/download)**. Recado lo abre y lo cierra solo, y descarga el modelo que necesita.
2. Descarga **`Recado.exe`** e **`instalar-ferramentas.ps1`** desde [Releases](../../releases).
3. *(Opcional, para audios)* Haz clic derecho en `instalar-ferramentas.ps1` y elige **Ejecutar con PowerShell**. Descarga whisper.cpp, un modelo de voz adecuado para tu RAM y, si falta, FFmpeg (con winget).
4. Abre `Recado.exe`. El panel se abre en el navegador con un código QR. En el teléfono, ve a **WhatsApp → Dispositivos vinculados → Vincular un dispositivo** y escanéalo.
5. En tu chat contigo mismo, o en un grupo donde solo estás tú (elígelo antes en el panel: *Configurações → Conversa do assistente*), envía `!ativar`. Recado responde y desde entonces solo escribe allí.

Los datos, el `config.toml` y los registros están en `%LOCALAPPDATA%\Recado`.

### PC recomendada

| RAM | Modelo elegido | Nota |
|---|---|---|
| 16 GB o más | `qwen3` (8B) | mejor análisis |
| 12 GB | `qwen3:4b` | buen equilibrio |
| 8 GB | `qwen3:1.7b` | más simple, pero útil |

La tarjeta gráfica no es obligatoria; sin ella, todo funciona con el procesador, solo que más lento. Para elegir tú mismo, pon `ajuste_automatico = false` en `config.toml`.

## Comandos (en el chat del asistente)

| Comando | Qué hace |
|---|---|
| `!ajuda` | lista de comandos |
| `!status` | comprueba que todo funcione |
| `!pendentes` | lo que esperan de ti |
| `!feito N` / `!cancelar N` | cierra o descarta el pendiente N |
| `!hoje` | resumen del día |
| `!resumo NOMBRE` | resumen de un chat (últimos 3 días) |
| `!buscar PALABRAS` | busca en todos los chats y fechas |
| `!pergunta TEXTO` | pregunta lo que quieras sobre tus mensajes |
| `!pausar` / `!retomar` | detiene o reanuda la lectura |
| `!desativar` | deja de enviar mensajes |

## Compilar el código

Necesitas Rust (toolchain `x86_64-pc-windows-gnu`) y MinGW (`windres`, que incrusta el ícono y los detalles del archivo).

```bash
cargo build --release
cargo test
```

El programa queda en `target/release/recado.exe`. Autopruebas sin WhatsApp: `recado.exe --testar-ia`, `--testar-audio archivo.ogg`, `--testar-espaco` y `--perguntar "pregunta"`.

## Créditos

Proyecto personal de **AjKaBaL**, compartido para ayudar a quien tuvo una idea parecida. Licencia [MIT](LICENSE).
