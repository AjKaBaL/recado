# Recado

**Assistente pessoal e privado para WhatsApp, rodando no seu próprio PC com Windows.** Ele lê as conversas
que você escolher, liga assuntos entre dias e te lembra do que importa, escrevendo **só para você**.

🌐 [English](README.md) · Português · [Español](README.es.md)

## O que ele faz

- **Alerta na hora** quando alguém pede algo para você, marca um prazo, cobra ou muda um horário. Ele avalia a conversa inteira; só citar seu nome não basta.
- **Pendências e lembretes.** Anota o que estão esperando de você, lembra na véspera do prazo e avisa quando o prazo passou. Também percebe quando algo foi resolvido depois ("te mando o orçamento" na segunda, e o orçamento chega na quinta).
- **Perguntas sobre o histórico.** Escreva `!pergunta quem ficou de me mandar o documento?`, ou qualquer pergunta, e ele procura em todas as conversas e datas.
- **Resumo diário**, **horário de silêncio** e **transcrição de áudios** no próprio computador.
- **Histórico antigo.** Pede ao seu celular mensagens mais antigas, de 50 em 50, até o começo da conversa.
- **Se ajusta ao PC.** Descobre a memória, a placa de vídeo NVIDIA e os núcleos do processador, escolhe o modelo de IA certo e o baixa pelo Ollama se estiver faltando.
- **Segundo número (opcional).** Mensagens que saem da sua própria conta nunca fazem o celular tocar. Se você vincular um segundo número de WhatsApp, os avisos chegam dele e o celular toca como numa conversa normal.
- **Painel local simples** em `http://127.0.0.1:7878` e ícone perto do relógio. Não fica nenhuma janela preta aberta.

## Regras de segurança (já embutidas)

- **Nunca** escreve para contatos, grupos ou outras pessoas. Só escreve para a conversa com você mesmo, ou para um grupo em que **só você** participa, e confere isso antes de cada envio. A IA nunca escolhe o destino.
- Nunca marca como lida, não arquiva, não apaga e não muda configurações do WhatsApp.
- Mensagens recebidas são tratadas como dados, nunca como ordens (proteção contra *prompt injection*).
- Tudo fica no seu PC: a IA roda localmente pelo [Ollama](https://ollama.com) e a transcrição pelo [whisper.cpp](https://github.com/ggml-org/whisper.cpp). Os logs nunca guardam texto de mensagens, QR Codes nem tokens.

> ⚠️ A conexão usa uma biblioteca **não oficial** do WhatsApp Web ([whatsapp-rust](https://crates.io/crates/whatsapp-rust)). Ela pode parar de funcionar quando o WhatsApp mudar, e não há garantia contra restrições na conta. Este projeto não tem ligação com o WhatsApp nem com a Meta.

## Instalação (Windows 10/11)

1. Instale o **[Ollama](https://ollama.com/download)**. O Recado abre e fecha o Ollama sozinho e baixa o modelo de que precisa.
2. Baixe o **`Recado.exe`** e o **`instalar-ferramentas.ps1`** em [Releases](../../releases).
3. *(Opcional, para áudios)* Clique com o botão direito em `instalar-ferramentas.ps1` e escolha **Executar com o PowerShell**. Ele baixa o whisper.cpp, um modelo de fala do tamanho certo para a sua memória e, se faltar, o FFmpeg (pelo winget).
4. Abra o `Recado.exe`. O painel abre no navegador com um QR Code. No celular, vá em **WhatsApp → Aparelhos conectados → Conectar aparelho** e leia o código.
5. Na conversa com você mesmo, ou num grupo em que só você participa (escolha o grupo antes no painel: *Configurações → Conversa do assistente*), mande `!ativar`. O Recado responde e, a partir daí, só escreve ali.

Os dados, o `config.toml` e os logs ficam em `%LOCALAPPDATA%\Recado`.

### Computador recomendado

| Memória | Modelo escolhido | Observação |
|---|---|---|
| 16 GB ou mais | `qwen3` (8B) | melhor análise |
| 12 GB | `qwen3:4b` | bom equilíbrio |
| 8 GB | `qwen3:1.7b` | mais simples, mas útil |

Placa de vídeo não é obrigatória; sem ela, tudo roda no processador, só que mais devagar. Para escolher você mesmo, ponha `ajuste_automatico = false` no `config.toml`.

## Comandos (na conversa do assistente)

| Comando | O que faz |
|---|---|
| `!ajuda` | lista os comandos |
| `!status` | mostra se está tudo funcionando |
| `!pendentes` | o que estão esperando de você |
| `!feito N` / `!cancelar N` | conclui ou descarta a pendência N |
| `!hoje` | resumo do dia |
| `!resumo NOME` | resumo de uma conversa (últimos 3 dias) |
| `!buscar PALAVRAS` | procura em todas as conversas e datas |
| `!pergunta TEXTO` | pergunte qualquer coisa sobre suas mensagens |
| `!pausar` / `!retomar` | para ou volta a ler |
| `!desativar` | para de enviar mensagens |

## Compilar o código

Precisa do Rust (toolchain `x86_64-pc-windows-gnu`) e do MinGW (`windres`, que coloca o ícone e os detalhes no `.exe`).

```bash
cargo build --release
cargo test
```

O programa sai em `target/release/recado.exe`. Autotestes sem WhatsApp: `recado.exe --testar-ia`, `--testar-audio arquivo.ogg`, `--testar-espaco` e `--perguntar "pergunta"`.

## Créditos

Projeto pessoal de **AjKaBaL**, compartilhado para ajudar quem teve uma ideia parecida. Licença [MIT](LICENSE).
