//! Camada de integração com o WhatsApp.
//!
//! É o ÚNICO módulo que conhece a biblioteca `whatsapp-rust` (não oficial).
//! O resto do aplicativo usa só os tipos deste módulo (`ConexaoWa`, `banco::Mensagem`),
//! assim a biblioteca pode ser trocada no futuro sem reconstruir o aplicativo.
//!
//! Regras de segurança aplicadas aqui:
//! - Nenhuma confirmação de leitura é enviada (a biblioteca só faz isso se pedirmos).
//! - Existe UMA função de envio (`enviar_ao_usuario`) e ela só aceita a própria conta
//!   ou o grupo escolhido pelo usuário, quando só ele participa desse grupo.

use crate::app::{AppRef, TarefaAudio, agora, mascarar};
use crate::banco::Mensagem;
use anyhow::{Result, anyhow, bail};
use std::sync::Arc;
use tracing::{info, warn};
use whatsapp_rust::prelude::*;

/// Alça da conexão ativa (cópia barata).
#[derive(Clone)]
pub struct ConexaoWa {
    cliente: Arc<Client>,
}

impl ConexaoWa {
    /// Identificadores da própria conta: (número, LID).
    pub fn conta_propria(&self) -> (Option<String>, Option<String>) {
        (
            self.cliente.pn().map(|j| j.to_non_ad_string()),
            self.cliente.lid().map(|j| j.to_non_ad_string()),
        )
    }

    pub fn nome_conta(&self) -> String {
        self.cliente.push_name()
    }

    /// Diz se `chat` é a conversa da própria conta ("mensagem para mim mesmo").
    pub fn eh_conversa_propria(&self, chat: &str) -> bool {
        let Ok(jid) = chat.parse::<Jid>() else { return false };
        eh_proprio(&jid, self.cliente.pn().as_ref(), self.cliente.lid().as_ref())
    }

    /// ÚNICA função de envio do aplicativo.
    ///
    /// Só envia para a conversa da própria conta OU para o grupo escolhido pelo
    /// usuário no painel (`grupo_autorizado`) — e, neste caso, só se o grupo tiver
    /// APENAS o próprio usuário como participante. Qualquer outro destino é recusado.
    /// O texto sempre começa com 🤖 para ficar claro que veio do assistente.
    pub async fn enviar_ao_usuario(
        &self,
        destino: &str,
        grupo_autorizado: Option<&str>,
        texto: &str,
    ) -> Result<String> {
        let jid: Jid = destino
            .parse()
            .map_err(|_| anyhow!("destino inválido"))?;
        let pn = self.cliente.pn();
        let lid = self.cliente.lid();
        if pn.is_none() && lid.is_none() {
            bail!("conta ainda não identificada");
        }
        let permitido = if eh_proprio(&jid, pn.as_ref(), lid.as_ref()) {
            true
        } else if matches!(jid.server, Server::Group) && grupo_autorizado == Some(destino) {
            match self.grupo_so_com_voce(&jid).await {
                Ok(true) => true,
                Ok(false) => {
                    warn!("BLOQUEADO: o grupo do assistente tem outros participantes");
                    bail!("o grupo tem outras pessoas; o assistente só envia para um grupo onde só você está");
                }
                Err(e) => bail!("não foi possível conferir os participantes do grupo: {e}"),
            }
        } else {
            false
        };
        if !permitido {
            warn!("BLOQUEADO: tentativa de envio para destino que não é do próprio usuário");
            bail!("destino recusado: só é permitido enviar para a própria conta ou para o grupo do assistente");
        }
        let texto = if texto.starts_with('🤖') {
            texto.to_string()
        } else {
            format!("🤖 {texto}")
        };
        let r = self
            .cliente
            .send_text(jid.to_non_ad(), texto)
            .await
            .map_err(|e| anyhow!("falha no envio: {e}"))?;
        Ok(r.message_id)
    }

    /// ÚNICA função de envio do SEGUNDO número: só manda para o número principal do
    /// usuário (lido da conexão principal, nunca da IA nem das mensagens). Recusa
    /// qualquer outro destino e recusa se as duas conexões forem a mesma conta.
    pub async fn enviar_ao_dono(&self, principal: &ConexaoWa, texto: &str) -> Result<String> {
        let Some(dono) = principal.cliente.pn().filter(|j| j.is_pn()) else {
            bail!("número principal ainda não identificado");
        };
        let dono = dono.to_non_ad();
        let (meu_pn, meu_lid) = (self.cliente.pn(), self.cliente.lid());
        if meu_pn.is_none() {
            bail!("segundo número ainda não identificado");
        }
        if eh_proprio(&dono, meu_pn.as_ref(), meu_lid.as_ref()) {
            bail!("o segundo número é o mesmo da conta principal");
        }
        let texto = if texto.starts_with('🤖') { texto.to_string() } else { format!("🤖 {texto}") };
        let r = self
            .cliente
            .send_text(dono, texto)
            .await
            .map_err(|e| anyhow!("falha no envio pelo segundo número: {e}"))?;
        Ok(r.message_id)
    }

    /// Verdadeiro se TODOS os participantes do grupo são a própria conta.
    pub async fn grupo_so_com_voce(&self, grupo: &Jid) -> Result<bool> {
        let meta = self
            .cliente
            .groups()
            .get_metadata(grupo)
            .await
            .map_err(|e| anyhow!("{e}"))?;
        let pn = self.cliente.pn();
        let lid = self.cliente.lid();
        let ids: Vec<Vec<Jid>> = meta
            .participants
            .iter()
            .map(|p| {
                let mut v = vec![p.jid.clone()];
                v.extend(p.phone_number.clone());
                v.extend(p.lid.clone());
                v
            })
            .collect();
        Ok(so_a_propria_conta(&ids, pn.as_ref(), lid.as_ref()))
    }

    /// Pede ao PRÓPRIO celular mensagens mais antigas de uma conversa (como o WhatsApp
    /// Web faz ao rolar para cima). Não envia nada para a conversa nem para outras pessoas:
    /// o pedido vai só para o celular do usuário, que responde com um pacote de histórico.
    pub async fn pedir_mensagens_antigas(
        &self,
        chat: &str,
        id_mais_antiga: &str,
        de_mim: bool,
        data_seg: i64,
        quantidade: i32,
    ) -> Result<()> {
        let jid: Jid = chat.parse().map_err(|_| anyhow!("conversa inválida"))?;
        self.cliente
            .fetch_message_history(&jid, id_mais_antiga, de_mim, data_seg * 1000, quantidade)
            .await
            .map_err(|e| anyhow!("o pedido não pôde ser enviado ao celular: {e}"))?;
        Ok(())
    }

    /// Número de telefone (só dígitos) de um contato que aparece como LID, se a
    /// biblioteca já souber (ela aprende isso com o tempo).
    pub async fn numero_do_lid(&self, jid: &str) -> Option<String> {
        let j: Jid = jid.parse().ok()?;
        self.cliente
            .get_lid_pn_entry(&j)
            .await
            .ok()
            .flatten()
            .map(|e| e.phone_number.to_string())
            .filter(|n| !n.is_empty())
    }

    /// Pede ao WhatsApp a lista completa de contatos salvos (nomes da agenda).
    /// Só lê: não muda nada na conta. Os nomes chegam como eventos `ContactUpdate`.
    pub async fn sincronizar_agenda(&self) {
        use whatsapp_rust::sync_task::MajorSyncTask;
        use whatsapp_rust::wacore::appstate::patch_decode::WAPatchName;
        self.cliente
            .process_sync_task(MajorSyncTask::AppStateSync { name: WAPatchName::CriticalUnblockLow, full_sync: true })
            .await;
    }

    /// Desconecta e remove este aparelho da lista de "aparelhos conectados".
    pub async fn sair(&self) {
        self.cliente.logout().await;
    }
}

/// Verdadeiro se `jid` é a própria conta (por número OU por LID), sem aparelho específico.
pub fn eh_proprio(jid: &Jid, pn: Option<&Jid>, lid: Option<&Jid>) -> bool {
    if jid.device != 0 || jid.user.is_empty() {
        return false;
    }
    match jid.server {
        Server::Pn => pn.is_some_and(|p| p.is_pn() && p.user == jid.user),
        Server::Lid => lid.is_some_and(|l| l.is_lid() && l.user == jid.user),
        _ => false,
    }
}

/// Cada participante é uma lista de identificadores (JID, número, LID).
/// Verdadeiro só se houver participantes e TODOS forem a própria conta.
pub fn so_a_propria_conta(participantes: &[Vec<Jid>], pn: Option<&Jid>, lid: Option<&Jid>) -> bool {
    !participantes.is_empty()
        && participantes.iter().all(|ids| {
            ids.iter().any(|j| {
                let j = j.to_non_ad();
                eh_proprio(&j, pn, lid)
            })
        })
}

/// Conversas que não interessam (status, canais, transmissões).
fn chat_ignorado(chat: &Jid) -> bool {
    !matches!(chat.server, Server::Pn | Server::Lid | Server::Group)
}

/// Tipo e texto de uma mensagem.
fn tipo_e_texto(msg: &wa::Message) -> (&'static str, Option<String>) {
    let base = msg.get_base_message();
    if let Some(t) = base.text_content() {
        return ("texto", Some(t.to_string()));
    }
    let legenda = base.get_caption().map(|s| s.to_string());
    if base.audio_message.as_option().is_some() {
        return ("audio", None);
    }
    if base.image_message.as_option().is_some() {
        return ("imagem", legenda);
    }
    if base.video_message.as_option().is_some() {
        return ("video", legenda);
    }
    if let Some(d) = base.document_message.as_option() {
        let nome = d.file_name.clone().or(d.title.clone());
        let t = match (nome, legenda) {
            (Some(n), Some(l)) => Some(format!("[documento: {n}] {l}")),
            (Some(n), None) => Some(format!("[documento: {n}]")),
            (None, l) => l,
        };
        return ("documento", t);
    }
    if base.sticker_message.as_option().is_some() {
        return ("figurinha", None);
    }
    if let Some(c) = base.contact_message.as_option() {
        return ("contato", c.display_name.clone().map(|n| format!("[contato: {n}]")));
    }
    if let Some(l) = base.location_message.as_option() {
        let t = l.name.clone().or(l.address.clone());
        return ("localizacao", Some(format!("[localização] {}", t.unwrap_or_default())));
    }
    ("outro", None)
}

fn normalizar(msg: &wa::Message, info: &MessageInfo) -> Option<Mensagem> {
    let chat = &info.source.chat;
    if chat_ignorado(chat) {
        return None;
    }
    let (tipo, texto) = tipo_e_texto(msg);
    if tipo == "outro" {
        // Reações, confirmações, mensagens de protocolo: não guardar.
        return None;
    }
    Some(Mensagem {
        chat: chat.to_non_ad_string(),
        id: info.id.to_string(),
        remetente: info.source.sender.to_non_ad_string(),
        nome_remetente: if info.source.is_from_me {
            "Eu".into()
        } else {
            info.push_name.clone()
        },
        de_mim: info.source.is_from_me,
        data: info.timestamp.timestamp(),
        tipo: tipo.into(),
        texto,
        origem: "ao_vivo".into(),
    })
}

/// Monta o bot com todos os tratadores de eventos e devolve-o pronto para rodar.
pub async fn construir(app: AppRef) -> Result<Bot> {
    let arquivo_sessao = app.pasta.join("sessao-whatsapp.db");
    preparar_releitura_da_agenda(&app, &arquivo_sessao);
    let sessao = SqliteStore::new(&arquivo_sessao.to_string_lossy())
        .await
        .map_err(|e| anyhow!("não foi possível abrir a sessão: {e}"))?;

    let app_ev = app.clone();
    let bot = Bot::builder()
        .with_backend(sessao)
        .on_event(move |evento, cliente| {
            let app = app_ev.clone();
            async move {
                tratar_evento(app, evento, cliente).await;
            }
        })
        .build()
        .await
        .map_err(|e| anyhow!("falha ao iniciar a conexão: {e}"))?;
    Ok(bot)
}

// ---------------------------------------------------------------- segundo número

/// Mantém a conexão do segundo número enquanto ele estiver ligado no painel.
/// Essa conexão NÃO lê nem guarda mensagens: serve só para mandar os avisos ao
/// número principal, para o celular tocar como numa conversa normal.
pub async fn rodar_segundo_numero(app: AppRef) {
    loop {
        // Sessões antigas (desvinculadas) são apagadas; se ainda estiverem em uso,
        // ficam para a próxima vez. Cada vinculação nova usa um arquivo novo.
        let atual = app.banco.estado("segundo_arquivo");
        if let Ok(rd) = std::fs::read_dir(&app.pasta) {
            for e in rd.filter_map(|e| e.ok()) {
                let nome = e.file_name().to_string_lossy().to_string();
                let em_uso = atual.as_deref().is_some_and(|a| nome.starts_with(a));
                if nome.starts_with("sessao-segundo-numero") && !em_uso {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        if !app.segundo_ligado() {
            // Guarda só o último aviso (ex.: "esse é o seu número principal").
            app.alterar_segundo(|e| *e = crate::app::EstadoSegundo { ultimo_evento: e.ultimo_evento.clone(), ..Default::default() });
            app.mudou_segundo.notified().await;
            continue;
        }
        let nome_arq = match atual {
            Some(a) => a,
            None => {
                let a = format!("sessao-segundo-numero-{}.db", agora());
                let _ = app.banco.definir_estado("segundo_arquivo", &a);
                a
            }
        };
        let arq = app.pasta.join(&nome_arq);
        app.alterar_segundo(|e| e.ultimo_evento = "Iniciando…".into());
        let sessao = match SqliteStore::new(&arq.to_string_lossy()).await {
            Ok(s) => s,
            Err(e) => {
                warn!("Segundo número: não foi possível abrir a sessão: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                continue;
            }
        };
        let app_ev = app.clone();
        let bot = Bot::builder()
            .with_backend(sessao)
            .on_event(move |evento, cliente| {
                let app = app_ev.clone();
                async move { tratar_evento_segundo(app, evento, cliente).await }
            })
            .build()
            .await;
        match bot {
            Ok(bot) => {
                let a = app.clone();
                tokio::select! {
                    _ = bot.run() => {}
                    // Desligado no painel: a conexão é encerrada.
                    _ = async { loop { a.mudou_segundo.notified().await; if !a.segundo_ligado() { break } } } => {}
                }
            }
            Err(e) => warn!("Segundo número: falha ao iniciar: {e}"),
        }
        if let Ok(mut g) = app.segundo.write() {
            *g = None;
        }
        app.alterar_segundo(|e| e.conectado = false);
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}

async fn tratar_evento_segundo(app: AppRef, evento: Arc<Event>, cliente: Arc<Client>) {
    match &*evento {
        Event::PairingQrCode(qr) => {
            app.alterar_segundo(|e| {
                e.qr = Some(qr.code.clone());
                e.ultimo_evento = "Leia o QR Code com o WhatsApp do SEGUNDO número".into();
            });
        }
        Event::PairSuccess(_) => app.alterar_segundo(|e| {
            e.qr = None;
            e.ultimo_evento = "Segundo número vinculado".into();
        }),
        Event::PairingQrCodesExhausted(_) => app.alterar_segundo(|e| {
            e.qr = None;
            e.ultimo_evento = "Os QR Codes expiraram. Desligue e ligue de novo para gerar outro.".into();
        }),
        Event::Connected(_) => {
            let con = ConexaoWa { cliente: cliente.clone() };
            let (pn, lid) = con.conta_propria();
            // Trava: não pode ser a mesma conta que o assistente lê.
            let principal = app.conexao().map(|c| c.conta_propria().0);
            if pn.is_some() && principal.as_ref().is_some_and(|p| p.as_ref() == pn.as_ref()) {
                warn!("Segundo número recusado: é a mesma conta principal");
                app.alterar_segundo(|e| {
                    e.conectado = false;
                    e.ultimo_evento = "Esse é o seu número principal. Use o WhatsApp de OUTRO número.".into();
                });
                con.sair().await;
                let _ = app.banco.definir_estado("segundo_numero", "0");
                let _ = app.banco.remover_estado("segundo_arquivo");
                app.mudou_segundo.notify_one();
                return;
            }
            for (k, v) in [("segundo_pn", &pn), ("segundo_lid", &lid)] {
                match v {
                    Some(j) => { let _ = app.banco.definir_estado(k, j); }
                    None => { let _ = app.banco.remover_estado(k); }
                }
            }
            if let Ok(mut g) = app.segundo.write() {
                *g = Some(con);
            }
            app.alterar_segundo(|e| {
                e.qr = None;
                e.conectado = true;
                e.conta_mascarada = pn.as_deref().map(|p| mascarar(p.split('@').next().unwrap_or(""))).unwrap_or_default();
                e.ultimo_evento = "Conectado — os avisos saem por este número".into();
            });
            info!("Segundo número conectado");
        }
        Event::Disconnected(_) => app.alterar_segundo(|e| {
            e.conectado = false;
            e.ultimo_evento = "Conexão perdida — reconectando…".into();
        }),
        Event::LoggedOut(_) => {
            app.alterar_segundo(|e| {
                e.conectado = false;
                e.ultimo_evento = "O segundo número foi desvinculado.".into();
            });
            let _ = app.banco.definir_estado("segundo_numero", "0");
            let _ = app.banco.remover_estado("segundo_arquivo");
            app.mudou_segundo.notify_one();
        }
        // Mensagens, histórico, contatos: ignorados de propósito (nada é lido nem guardado).
        _ => {}
    }
}

async fn tratar_evento(app: AppRef, evento: Arc<Event>, cliente: Arc<Client>) {
    if app.conexao().is_none() {
        app.definir_conexao(ConexaoWa { cliente: cliente.clone() });
    }
    match &*evento {
        Event::PairingQrCode(qr) => {
            app.alterar_estado(|e| {
                e.qr = Some(qr.code.clone());
                e.logado = false;
                e.ultimo_evento = "Aguardando leitura do QR Code".into();
            });
            info!("Novo QR Code disponível no painel");
        }
        Event::PairSuccess(_) => {
            app.alterar_estado(|e| {
                e.qr = None;
                e.ultimo_evento = "Aparelho vinculado com sucesso".into();
            });
            info!("Aparelho vinculado");
        }
        Event::PairError(_) => {
            app.alterar_estado(|e| e.ultimo_evento = "Falha ao vincular. Tente novamente.".into());
            warn!("Falha no pareamento");
        }
        Event::PairingQrCodesExhausted(_) => {
            app.alterar_estado(|e| {
                e.qr = None;
                e.ultimo_evento =
                    "Os QR Codes expiraram. Feche e abra o assistente para gerar outro.".into();
            });
        }
        Event::Connected(_) => {
            let con = ConexaoWa { cliente: cliente.clone() };
            let (pn, _lid) = con.conta_propria();
            let nome = con.nome_conta();
            app.definir_conexao(con);
            app.alterar_estado(|e| {
                e.qr = None;
                e.conectado = true;
                e.logado = true;
                e.nome_conta = nome;
                e.conta_mascarada = pn
                    .map(|p| mascarar(p.split('@').next().unwrap_or("")))
                    .unwrap_or_default();
                e.ultimo_evento = "Conectado".into();
            });
            info!("Conectado ao WhatsApp");
            // Nomes: completa com o que já está guardado, procura números dos contatos
            // ocultos e, uma vez, busca os nomes da agenda.
            let app2 = app.clone();
            let con = ConexaoWa { cliente: cliente.clone() };
            tokio::spawn(async move { atualizar_nomes(&app2, &con, false).await });
        }
        Event::ContactUpdate(c) => {
            let nome = c
                .action
                .full_name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .or(c.action.first_name.clone())
                .unwrap_or_default();
            let mut jids = vec![c.jid.to_non_ad_string()];
            for extra in [&c.action.lid_jid, &c.action.pn_jid].into_iter().flatten() {
                if let Ok(j) = extra.parse::<Jid>() {
                    jids.push(j.to_non_ad_string());
                }
            }
            let _ = app.banco.definir_nome_agenda(&jids, &nome);
        }
        Event::PushNameUpdate(p) => {
            let _ = app.banco.definir_nome_whatsapp(&p.jid.to_non_ad_string(), &p.new_push_name, None);
        }
        Event::Disconnected(_) => {
            app.alterar_estado(|e| {
                e.conectado = false;
                e.ultimo_evento = "Conexão perdida — reconectando automaticamente".into();
            });
            warn!("Desconectado; a biblioteca tentará reconectar");
        }
        Event::LoggedOut(_) => {
            app.alterar_estado(|e| {
                e.conectado = false;
                e.logado = false;
                e.ultimo_evento =
                    "Este computador foi desconectado do WhatsApp. Feche e abra o assistente para conectar de novo."
                        .into();
            });
            let _ = app.banco.remover_estado("envio_habilitado");
            // A sessão não vale mais: apagar para gerar um QR Code novo na próxima conexão.
            let _ = std::fs::write(app.pasta.join("apagar-sessao.marca"), b"1");
            warn!("Sessão encerrada (logout)");
        }
        Event::ClientOutdated(_) => {
            app.alterar_estado(|e| {
                e.ultimo_evento =
                    "O WhatsApp recusou esta versão da biblioteca. É preciso atualizar o assistente."
                        .into()
            });
            warn!("Cliente desatualizado");
        }
        Event::Messages(lote) => {
            for item in lote.iter() {
                tratar_mensagem(&app, &cliente, &item.message, &item.info).await;
            }
        }
        Event::HistorySync(hs) => {
            let progresso = hs.progress();
            app.alterar_estado(|e| e.historico_progresso = progresso);
            let hs = hs.clone();
            let app2 = app.clone();
            tokio::task::spawn_blocking(move || importar_historico(&app2, &hs));
        }
        _ => {}
    }
}

async fn tratar_mensagem(app: &AppRef, cliente: &Arc<Client>, msg: &wa::Message, info: &MessageInfo) {
    let Some(m) = normalizar(msg, info) else { return };
    let con = ConexaoWa { cliente: cliente.clone() };
    // Conversa com o segundo número: não é guardada nem analisada. Só os comandos
    // que VOCÊ escreve nela são atendidos (a resposta volta pelo segundo número).
    if app.eh_chat_do_segundo(&m.chat) {
        if m.de_mim
            && let Some(t) = m.texto.as_deref()
            && t.trim_start().starts_with(&app.cfg.prefixo_comando)
        {
            crate::comandos::executar(app.clone(), con, m.chat.clone(), t.to_string()).await;
        }
        return;
    }
    let conversa_propria = con.eh_conversa_propria(&m.chat);

    // Nome da conversa: grupos pelo assunto do grupo; contatos pelo nome exibido.
    let nome = if info.source.is_group {
        nome_grupo(app, cliente, &info.source.chat).await
    } else if conversa_propria {
        Some("Eu (mensagens para mim)".to_string())
    } else {
        None
    };
    let _ = app.banco.registrar_conversa(&m.chat, nome.as_deref(), info.source.is_group);
    app.alterar_estado(|e| e.ultima_msg = Some(agora()));
    // Nome que a pessoa pôs no WhatsApp (vale também para mensagens dela em grupos).
    if !m.de_mim && !info.push_name.trim().is_empty() {
        let numero = if m.remetente.ends_with("@lid") {
            con.numero_do_lid(&m.remetente).await
        } else {
            m.remetente.split('@').next().map(str::to_string)
        };
        let _ = app.banco.definir_nome_whatsapp(&m.remetente, &info.push_name, numero.as_deref());
    }
    if info.source.is_group {
        app.talvez_adotar_grupo(&m.chat, &app.banco.nome_conversa(&m.chat));
    }

    // Comandos na conversa do assistente (consigo mesmo ou o grupo escolhido), só meus.
    if m.de_mim && app.eh_conversa_do_assistente(&con, &m.chat) {
        if app.banco.foi_enviada_pelo_assistente(&m.id)
            || m.texto.as_deref().is_some_and(|t| t.starts_with('🤖'))
        {
            return; // eco de mensagem do próprio assistente: ignorar
        }
        if let Some(t) = m.texto.as_deref()
            && t.trim_start().starts_with(&app.cfg.prefixo_comando)
        {
            let chat = m.chat.clone();
            let texto = t.to_string();
            crate::comandos::executar(app.clone(), con, chat, texto).await;
            return;
        }
    }

    if app.pausado() || !app.banco.conversa_monitorada(&m.chat) {
        return;
    }
    match app.banco.salvar_mensagem(&m) {
        Ok(true) => {}
        Ok(false) => return, // duplicada
        Err(e) => {
            warn!("Erro ao gravar mensagem: {e}");
            return;
        }
    }

    // Áudio: baixar e mandar para a fila de transcrição.
    if m.tipo == "audio" && app.cfg.transcricao.ativada {
        let base = msg.get_base_message();
        if let Some(audio) = base.audio_message.as_option() {
            match cliente.download(audio).await {
                Ok(bytes) => {
                    let dir = app.pasta.join("audios");
                    let _ = std::fs::create_dir_all(&dir);
                    let nome: String = m.id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                    let arq = dir.join(format!("{nome}.ogg"));
                    if let Err(e) = std::fs::write(&arq, bytes) {
                        let _ = app.banco.registrar_audio(&m.chat, &m.id, None, "falhou", Some(&e.to_string()));
                        return;
                    }
                    let _ = app.banco.registrar_audio(
                        &m.chat, &m.id, Some(&arq.to_string_lossy()), "pendente", None,
                    );
                    let _ = app
                        .fila_audio
                        .send(TarefaAudio { chat: m.chat.clone(), id: m.id.clone(), arquivo: arq })
                        .await;
                }
                Err(e) => {
                    warn!("Não foi possível baixar um áudio: {e}");
                    let _ = app.banco.registrar_audio(&m.chat, &m.id, None, "falhou", Some("download falhou"));
                    let _ = app.banco.definir_texto(&m.chat, &m.id, "[áudio não pôde ser baixado]");
                }
            }
        }
    }
}

/// Uma vez (ou quando pedido), faz a biblioteca baixar de novo a lista completa de
/// contatos: apaga só a versão guardada da coleção "critical_unblock_low" (onde ficam
/// os contatos). Com versão 0, a biblioteca pede a cópia completa e substitui a antiga.
fn preparar_releitura_da_agenda(app: &AppRef, arquivo_sessao: &std::path::Path) {
    if app.banco.estado("agenda_lida_v2").is_some() || !arquivo_sessao.exists() {
        return;
    }
    let r = rusqlite::Connection::open(arquivo_sessao).and_then(|c| {
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        c.execute("DELETE FROM app_state_versions WHERE name='critical_unblock_low'", [])
    });
    match r {
        Ok(_) => {
            let _ = app.banco.definir_estado("agenda_pedir", "1");
            info!("A lista de contatos será baixada de novo ao conectar");
        }
        Err(e) => warn!("Não foi possível preparar a leitura da agenda: {e}"),
    }
}

/// Atualiza nomes e números dos contatos. `forcar_agenda` pede a agenda de novo.
pub async fn atualizar_nomes(app: &AppRef, con: &ConexaoWa, forcar_agenda: bool) {
    let _ = app.banco.completar_nomes();
    if let Ok(lids) = app.banco.contatos_sem_numero() {
        let mut achados = 0;
        for jid in lids {
            if let Some(n) = con.numero_do_lid(&jid).await {
                let _ = app.banco.definir_numero(&jid, &n);
                achados += 1;
            }
        }
        info!("Números encontrados para {achados} contatos ocultos");
    }
    if forcar_agenda || app.banco.estado("agenda_pedir").is_some() {
        info!("Pedindo os nomes da agenda ao WhatsApp");
        con.sincronizar_agenda().await;
        // Os nomes chegam como eventos logo depois; dá um tempo e completa.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let _ = app.banco.remover_estado("agenda_pedir");
        let _ = app.banco.definir_estado("agenda_lida_v2", &agora().to_string());
        let _ = app.banco.completar_nomes();
        let n: i64 = app
            .banco
            .conexao()
            .query_row("SELECT COUNT(*) FROM agenda", [], |r| r.get(0))
            .unwrap_or(0);
        info!("Nomes da agenda atualizados ({n} contatos na agenda)");
    }
}

async fn nome_grupo(app: &AppRef, cliente: &Arc<Client>, chat: &Jid) -> Option<String> {
    let chave = chat.to_non_ad_string();
    let atual = app.banco.nome_conversa(&chave);
    if !atual.is_empty() && atual != chave.split('@').next().unwrap_or("") {
        return None; // já temos nome
    }
    match cliente.groups().get_metadata(chat).await {
        Ok(meta) => Some(meta.subject),
        Err(_) => None,
    }
}

/// Importa mensagens do histórico enviado pelo celular ao vincular.
fn importar_historico(app: &AppRef, hs: &whatsapp_rust::wacore::types::events::LazyHistorySync) {
    let Some(h) = hs.get() else {
        warn!("Pacote de histórico não pôde ser lido");
        return;
    };
    let mut total = 0usize;
    let pausado = app.pausado();
    if pausado {
        return;
    }
    for conv in &h.conversations {
        let Ok(chat) = conv.id.parse::<Jid>() else { continue };
        if chat_ignorado(&chat) {
            continue;
        }
        let chat_s = chat.to_non_ad_string();
        let grupo = matches!(chat.server, Server::Group);
        let _ = app.banco.registrar_conversa(&chat_s, conv.name.as_deref(), grupo);
        if grupo && let Some(n) = conv.name.as_deref() {
            app.talvez_adotar_grupo(&chat_s, n);
        }
        if !app.banco.conversa_monitorada(&chat_s) {
            continue;
        }
        let mut lote = Vec::new();
        for hm in &conv.messages {
            let Some(wmi) = hm.message.as_option() else { continue };
            let Some(key) = wmi.key.as_option() else { continue };
            let Some(corpo) = wmi.message.as_option() else { continue };
            let (tipo, texto) = tipo_e_texto(corpo);
            if tipo == "outro" {
                continue;
            }
            let de_mim = key.from_me.unwrap_or(false);
            let remetente = if de_mim {
                "eu".to_string()
            } else if grupo {
                wmi.participant.clone().or(key.participant.clone()).unwrap_or_default()
            } else {
                chat_s.clone()
            };
            lote.push(Mensagem {
                chat: chat_s.clone(),
                id: key.id.clone().unwrap_or_default(),
                remetente,
                nome_remetente: if de_mim { "Eu".into() } else { wmi.push_name.clone().unwrap_or_default() },
                de_mim,
                data: wmi.message_timestamp.unwrap_or(0) as i64,
                tipo: tipo.into(),
                texto: if tipo == "audio" { Some("[áudio do histórico — não transcrito]".into()) } else { texto },
                origem: "historico".into(),
            });
        }
        lote.retain(|m| !m.id.is_empty());
        match app.banco.salvar_lote(&lote) {
            Ok(n) => total += n,
            Err(e) => warn!("Erro ao gravar histórico: {e}"),
        }
    }
    for p in &h.pushnames {
        if let (Some(id), Some(nome)) = (p.id.as_deref(), p.pushname.as_deref())
            && let Ok(j) = id.parse::<Jid>()
        {
            let _ = app.banco.definir_nome_whatsapp(&j.to_non_ad_string(), nome, None);
        }
    }
    app.historico_recebido.store(crate::app::agora_ms(), std::sync::atomic::Ordering::Relaxed);
    info!("Histórico: {total} mensagens novas importadas");
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn so_aceita_a_propria_conta() {
        let pn: Jid = "5511999990000@s.whatsapp.net".parse().unwrap();
        let lid: Jid = "123456789@lid".parse().unwrap();
        let ok_pn: Jid = "5511999990000@s.whatsapp.net".parse().unwrap();
        let ok_lid: Jid = "123456789@lid".parse().unwrap();
        let outro: Jid = "5511888880000@s.whatsapp.net".parse().unwrap();
        let grupo: Jid = "5511999990000-1600000000@g.us".parse().unwrap();
        let mesmo_num_lid: Jid = "5511999990000@lid".parse().unwrap();
        let aparelho: Jid = "5511999990000:3@s.whatsapp.net".parse().unwrap();

        assert!(eh_proprio(&ok_pn, Some(&pn), Some(&lid)));
        assert!(eh_proprio(&ok_lid, Some(&pn), Some(&lid)));
        assert!(!eh_proprio(&outro, Some(&pn), Some(&lid)));
        assert!(!eh_proprio(&grupo, Some(&pn), Some(&lid)));
        assert!(!eh_proprio(&mesmo_num_lid, Some(&pn), Some(&lid)));
        assert!(!eh_proprio(&aparelho, Some(&pn), Some(&lid)));
        assert!(!eh_proprio(&ok_pn, None, None));
    }

    #[test]
    fn grupo_so_com_a_propria_conta() {
        let pn: Jid = "5511999990000@s.whatsapp.net".parse().unwrap();
        let lid: Jid = "123456789@lid".parse().unwrap();
        let eu_lid: Vec<Jid> = vec!["123456789@lid".parse().unwrap()];
        let eu_aparelho: Vec<Jid> = vec!["5511999990000:2@s.whatsapp.net".parse().unwrap()];
        let outro: Vec<Jid> = vec![
            "987654321@lid".parse().unwrap(),
            "5511888880000@s.whatsapp.net".parse().unwrap(),
        ];
        assert!(so_a_propria_conta(&[eu_lid.clone()], Some(&pn), Some(&lid)));
        assert!(so_a_propria_conta(&[eu_aparelho], Some(&pn), Some(&lid)));
        assert!(!so_a_propria_conta(&[eu_lid, outro], Some(&pn), Some(&lid)));
        assert!(!so_a_propria_conta(&[], Some(&pn), Some(&lid)));
    }
}
