//! Comandos digitados pelo usuário na conversa do assistente (consigo mesmo ou o grupo dele).

use crate::app::{AppRef, agora, data_curta};
use crate::whatsapp::ConexaoWa;
use crate::{notificacao, resumos};
use tracing::{info, warn};

fn ajuda(p: &str) -> String {
    format!(
        "🤖 *Recado — comandos*\n\
{p}ajuda — esta lista\n\
{p}status — situação do assistente\n\
{p}pendentes — o que estão esperando de você\n\
{p}feito N — marca a pendência N como concluída\n\
{p}cancelar N — descarta a pendência N\n\
{p}hoje — resumo do dia até agora\n\
{p}resumo NOME — resumo de uma conversa (últimos 3 dias)\n\
{p}buscar PALAVRAS — procura em todas as conversas e datas\n\
{p}pergunta TEXTO — pergunte qualquer coisa (ex.: {p}pergunta quem ficou de me mandar o documento?)\n\
{p}pausar / {p}retomar — para ou volta a ler as conversas\n\
{p}desativar — para de enviar mensagens aqui\n\n\
Dica: escrever {p} seguido de uma pergunta qualquer também funciona."
    )
}

/// Executa um comando. Só é chamado para mensagens de MIM, na conversa do assistente.
pub async fn executar(app: AppRef, con: ConexaoWa, chat: String, texto: String) {
    let p = app.cfg.prefixo_comando.clone();
    let corpo = texto.trim_start().trim_start_matches(p.as_str()).trim().to_string();
    let (cmd, resto) = match corpo.split_once(char::is_whitespace) {
        Some((c, r)) => (c.to_lowercase(), r.trim().to_string()),
        None => (corpo.to_lowercase(), String::new()),
    };
    info!("Comando recebido: {cmd}");

    // Ativação: a própria mensagem prova qual é a conversa consigo mesmo.
    if cmd == "ativar" {
        if !app.eh_conversa_do_assistente(&con, &chat) {
            warn!("Ativação recusada: não é a conversa escolhida para o assistente");
            return;
        }
        let _ = app.banco.remover_estado("erro_ativacao");
        let _ = app.banco.definir_estado("chat_proprio", &chat);
        let _ = app.banco.definir_estado("envio_habilitado", "1");
        let msg = format!(
            "✅ Envio ativado! A partir de agora eu aviso aqui o que for importante.\n\
Só envio mensagens para ESTA conversa — nunca para outras pessoas.\n\n{}",
            ajuda(&p)
        );
        match notificacao::enviar(&app, &msg).await {
            Ok(()) => {
                let _ = app.banco.definir_estado("envio_validado_em", &agora().to_string());
                info!("Envio para a própria conversa validado");
            }
            Err(e) => {
                warn!("Teste de envio falhou: {e}");
                let _ = app.banco.definir_estado("erro_ativacao", &e.to_string());
                let _ = app.banco.remover_estado("envio_habilitado");
            }
        }
        return;
    }

    if !app.envio_habilitado() {
        // Sem ativação, não respondemos nada (nada é enviado).
        return;
    }

    let resposta: String = match cmd.as_str() {
        "ajuda" | "help" | "comandos" | "" => ajuda(&p),
        "status" => status(&app),
        "pausar" => {
            app.definir_pausado(true);
            format!("⏸️ Pausado. Não vou ler nem guardar mensagens até você mandar {p}retomar.")
        }
        "retomar" => {
            app.definir_pausado(false);
            "▶️ Voltei a acompanhar as conversas.".into()
        }
        "desativar" => {
            let _ = notificacao::enviar(&app, &format!("🔕 Envio desativado. Para reativar, mande {p}ativar aqui.")).await;
            let _ = app.banco.remover_estado("envio_habilitado");
            return;
        }
        "pendentes" | "pendencias" | "pendências" | "lembretes" => {
            match resumos::texto_pendencias(&app, 30) {
                Ok(t) => format!("📌 *Pendências*\n{t}"),
                Err(e) => format!("Erro ao ler pendências: {e}"),
            }
        }
        "feito" | "concluido" | "concluído" | "ok" => mudar_item(&app, &resto, "concluido"),
        "cancelar" | "descartar" => mudar_item(&app, &resto, "cancelado"),
        "buscar" | "procurar" => buscar(&app, &resto),
        "hoje" => em_segundo_plano(&app, move |a| resumos::resumo_do_dia(a)).await,
        "resumo" | "resumir" => {
            if resto.is_empty() {
                format!("Diga de qual conversa. Ex.: {p}resumo Grupo da Faculdade")
            } else {
                em_segundo_plano(&app, move |a| resumos::resumo_conversa(a, &resto, 3)).await
            }
        }
        "pergunta" | "perguntar" => {
            if resto.is_empty() {
                format!("Escreva a pergunta. Ex.: {p}pergunta alguém pediu algo para mim hoje?")
            } else {
                em_segundo_plano(&app, move |a| resumos::responder_pergunta(a, &resto)).await
            }
        }
        _ => {
            // Qualquer outra coisa depois do prefixo é tratada como pergunta.
            let pergunta = corpo.clone();
            em_segundo_plano(&app, move |a| resumos::responder_pergunta(a, &pergunta)).await
        }
    };
    if let Err(e) = notificacao::enviar(&app, &resposta).await {
        warn!("Não foi possível responder ao comando: {e}");
    }
}

async fn em_segundo_plano<F>(app: &AppRef, f: F) -> String
where
    F: FnOnce(&crate::app::App) -> anyhow::Result<String> + Send + 'static,
{
    let a = app.clone();
    match tokio::task::spawn_blocking(move || f(&a)).await {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => format!("Não consegui concluir: {e}"),
        Err(_) => "Não consegui concluir (tarefa interrompida).".into(),
    }
}

fn status(app: &AppRef) -> String {
    let c = app.banco.contadores();
    let e = app.ler_estado();
    format!(
        "📊 *Status*\n\
Conexão: {}\n\
Leitura: {}\n\
IA local: {}\n\
Conversas: {} ({} monitoradas)\n\
Mensagens guardadas: {}\n\
Áudios transcritos: {} (na fila: {})\n\
Painel: http://127.0.0.1:{}",
        if e.conectado { "conectado ✅" } else { "desconectado ❌" },
        if app.pausado() { "pausada ⏸️" } else { "ativa ▶️" },
        if app.ia_ok() { "funcionando ✅" } else { "indisponível ❌" },
        c.conversas, c.monitoradas, c.mensagens, c.audios_transcritos, c.audios_pendentes,
        app.cfg.porta_painel
    )
}

fn mudar_item(app: &AppRef, resto: &str, estado: &str) -> String {
    let ids: Vec<i64> = resto
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse().ok())
        .collect();
    if ids.is_empty() {
        return format!("Diga o número. Ex.: {}feito 12", app.cfg.prefixo_comando);
    }
    let c = app.banco.conexao();
    let mut ok = Vec::new();
    for id in ids {
        if c.execute("UPDATE itens SET estado=?2 WHERE id=?1 AND estado='pendente'", rusqlite::params![id, estado])
            .unwrap_or(0) > 0
        {
            ok.push(id.to_string());
        }
    }
    if ok.is_empty() {
        "Não achei pendência aberta com esse número.".into()
    } else if estado == "concluido" {
        format!("✅ Concluído: {}", ok.join(", "))
    } else {
        format!("🗑️ Descartado: {}", ok.join(", "))
    }
}

fn buscar(app: &AppRef, termos: &str) -> String {
    if termos.is_empty() {
        return format!("Diga o que procurar. Ex.: {}buscar orçamento", app.cfg.prefixo_comando);
    }
    match app.banco.buscar(termos, 12) {
        Ok(v) if v.is_empty() => format!("🔎 Nada encontrado para \"{termos}\"."),
        Ok(v) => {
            let mut s = format!("🔎 *Resultados para \"{termos}\"*\n");
            for m in v {
                let t: String = m.texto.unwrap_or_default().chars().take(160).collect();
                let quem = if m.de_mim { "Eu".to_string() } else { m.nome_remetente };
                s.push_str(&format!("\n• {} — {} — {quem}: {t}", m.nome_chat, data_curta(m.data)));
            }
            s
        }
        Err(e) => format!("Erro na busca: {e}"),
    }
}
