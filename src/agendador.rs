//! Tarefas em segundo plano: análise das conversas, lembretes, resumo diário.

use crate::app::{AppRef, agora, data_curta};
use crate::{analise, notificacao, resumos};
use crate::ia::ProvedorIa;
use chrono::{Local, NaiveTime};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tracing::{info, warn};

pub async fn rodar(app: AppRef) {
    let mut ultima_verificacao_ia = 0i64;
    loop {
        // Verifica a IA a cada 5 minutos.
        if app.cfg.ia.ativada && agora() - ultima_verificacao_ia > 300 {
            ultima_verificacao_ia = agora();
            let a = app.clone();
            // Se o Ollama fechou ou travou, abre de novo.
            let ok = tokio::task::spawn_blocking(move || {
                if crate::ollama::garantir(&a, Duration::from_secs(90)) {
                    Ok(())
                } else {
                    resumos::provedor(&a).disponivel().map_err(|e| e.to_string())
                }
            })
            .await
            .ok();
            let disponivel = matches!(ok, Some(Ok(())));
            if disponivel != app.ia_disponivel.swap(disponivel, Ordering::Relaxed) {
                if disponivel {
                    info!("IA local disponível");
                } else if let Some(Err(e)) = ok {
                    warn!("IA local indisponível: {e}");
                }
            }
        }

        if !app.pausado() {
            if app.ia_ok() {
                analisar_pendentes(&app).await;
            }
            if app.envio_habilitado() {
                lembretes(&app).await;
                resumo_diario(&app).await;
                notificacao::liberar_adiados(&app).await;
            }
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

async fn analisar_pendentes(app: &AppRef) {
    let espera = app.cfg.ia.espera_analise_seg as i64;
    let chats = match app.banco.conversas_para_analisar(agora(), espera) {
        Ok(c) => c,
        Err(e) => {
            warn!("Erro ao buscar conversas para analisar: {e}");
            return;
        }
    };
    for chat in chats.into_iter().take(5) {
        let a = app.clone();
        let nome = {
            let n = app.ler_estado().nome_conta;
            if n.trim().is_empty() { "o usuário".to_string() } else { n }
        };
        let c2 = chat.clone();
        let r = tokio::task::spawn_blocking(move || {
            let ia = resumos::provedor(&a);
            analise::analisar_conversa(&a, &ia as &dyn ProvedorIa, &c2, &nome)
        })
        .await;
        match r {
            Ok(Ok(res)) => {
                if res.analisadas > 0 {
                    info!("Conversa analisada ({} mensagens)", res.analisadas);
                }
                if let Some(alerta) = res.alerta
                    && app.cfg.ia.alertas_imediatos
                    && (app.envio_habilitado() || notificacao::na_tela_ligada(app))
                    && let Err(e) = notificacao::avisar(app, &alerta, res.critico).await
                {
                    warn!("Não foi possível enviar alerta: {e}");
                }
            }
            Ok(Err(e)) => {
                warn!("Falha ao analisar conversa: {e}");
                // Evita repetir infinitamente uma conversa que sempre falha.
                let falhas_chave = format!("falhas_analise:{chat}");
                let n: u32 = app.banco.estado(&falhas_chave).and_then(|s| s.parse().ok()).unwrap_or(0) + 1;
                let _ = app.banco.definir_estado(&falhas_chave, &n.to_string());
                if n >= 3 {
                    if let Ok(msgs) = app.banco.nao_analisadas(&chat, 60) {
                        let ids: Vec<String> = msgs.into_iter().map(|m| m.id).collect();
                        let _ = app.banco.marcar_analisadas(&chat, &ids);
                    }
                    let _ = app.banco.remover_estado(&falhas_chave);
                }
                break;
            }
            Err(e) => warn!("Tarefa de análise interrompida: {e}"),
        }
    }
}

async fn lembretes(app: &AppRef) {
    let agora = agora();
    let itens: Vec<(i64, String, i64, i64, String)> = {
        let c = app.banco.conexao();
        let Ok(mut st) = c.prepare(
            "SELECT id, descricao, prazo, lembrado, chat FROM itens
             WHERE estado='pendente' AND prazo IS NOT NULL
               AND ((lembrado=0 AND prazo - ?1 <= 86400) OR (lembrado<=1 AND prazo <= ?1))",
        ) else {
            return;
        };
        st.query_map([agora], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .map(|it| it.filter_map(Result::ok).collect())
            .unwrap_or_default()
    };
    for (id, desc, prazo, _lembrado, chat) in itens {
        let atrasado = prazo <= agora;
        let txt = if atrasado {
            format!("⌛ *Prazo passou*: [{id}] {desc}\nEra até {} ({}).\nResponda {}feito {id} se já resolveu.",
                data_curta(prazo), app.banco.nome_conversa(&chat), app.cfg.prefixo_comando)
        } else {
            format!("⏰ *Lembrete*: [{id}] {desc}\nPrazo: {} ({}).", data_curta(prazo), app.banco.nome_conversa(&chat))
        };
        if notificacao::avisar(app, &txt, false).await.is_ok() {
            let novo = if atrasado { 2 } else { 1 };
            let _ = app.banco.conexao().execute("UPDATE itens SET lembrado=?2 WHERE id=?1", [id, novo]);
        }
    }
}

async fn resumo_diario(app: &AppRef) {
    let horario = app.cfg.ia.horario_resumo_diario.trim();
    let Ok(hora) = NaiveTime::parse_from_str(horario, "%H:%M") else { return };
    let agora_local = Local::now();
    if agora_local.time() < hora {
        return;
    }
    let hoje = agora_local.format("%Y-%m-%d").to_string();
    if app.banco.estado("ultimo_resumo_diario").as_deref() == Some(hoje.as_str()) {
        return;
    }
    let a = app.clone();
    match tokio::task::spawn_blocking(move || resumos::resumo_do_dia(&a)).await {
        Ok(Ok(txt)) => {
            if notificacao::enviar(app, &txt).await.is_ok() {
                let _ = app.banco.definir_estado("ultimo_resumo_diario", &hoje);
            }
        }
        Ok(Err(e)) => warn!("Falha ao gerar resumo diário: {e}"),
        Err(e) => warn!("Tarefa de resumo interrompida: {e}"),
    }
}
