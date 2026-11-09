//! Envio de avisos para a conversa do próprio usuário.
//!
//! O destino NUNCA é escolhido aqui nem pela IA: ele vem da conversa escolhida
//! pelo usuário (consigo mesmo ou um grupo só dele) e validada pelo comando de
//! ativação, e a camada `whatsapp` confere de novo antes de cada envio.

use crate::app::{App, agora};
use anyhow::{Result, bail};
use chrono::{Local, NaiveTime};
use tracing::{info, warn};

/// Envia um aviso para o próprio usuário. Se estiver no horário de silêncio e
/// não for crítico, guarda para mandar depois.
pub async fn avisar(app: &App, texto: &str, critico: bool) -> Result<()> {
    if !critico && em_silencio(&app.cfg.ia.silencio) {
        adiar(app, texto);
        return Ok(());
    }
    let na_tela = mostrar_na_tela(app, texto).await;
    if !app.envio_habilitado() {
        if na_tela {
            return Ok(());
        }
        bail!("envio pelo WhatsApp ainda não foi ativado");
    }
    match enviar(app, texto).await {
        Err(e) if !na_tela => Err(e),
        _ => Ok(()),
    }
}

/// As notificações do Windows estão ligadas? (padrão: não; muda no painel)
pub fn na_tela_ligada(app: &App) -> bool {
    app.banco.estado("notificar_windows").as_deref() == Some("1")
}

/// Mostra o aviso também como notificação do Windows. Devolve true se apareceu.
pub async fn mostrar_na_tela(app: &App, texto: &str) -> bool {
    if !na_tela_ligada(app) {
        return false;
    }
    let (titulo, corpo) = texto_da_notificacao(texto);
    let pasta = app.pasta.clone();
    let url = format!("http://127.0.0.1:{}", app.cfg.porta_painel);
    match tokio::task::spawn_blocking(move || crate::sistema::notificar(&pasta, &titulo, &corpo, &url)).await {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            warn!("{e:#}");
            false
        }
        Err(_) => false,
    }
}

/// Primeira linha vira o título; o resto (curto) vira o texto. Tira os * do WhatsApp.
fn texto_da_notificacao(texto: &str) -> (String, String) {
    let limpo = texto.replace('*', "");
    let mut linhas = limpo.lines().map(str::trim).filter(|l| !l.is_empty());
    let primeira = linhas.next().unwrap_or("Aviso do assistente");
    let resto: Vec<&str> = linhas.collect();
    let cortar = |s: &str, n: usize| -> String {
        if s.chars().count() > n { s.chars().take(n - 1).collect::<String>() + "…" } else { s.to_string() }
    };
    if resto.is_empty() {
        ("Recado".into(), cortar(primeira, 250))
    } else {
        (cortar(primeira, 80), cortar(&resto.join("\n"), 250))
    }
}

/// Envio imediato (respostas a comandos e avisos já liberados).
pub async fn enviar(app: &App, texto: &str) -> Result<()> {
    if !app.envio_habilitado() {
        bail!("envio pelo WhatsApp ainda não foi ativado");
    }
    let Some(destino) = app.banco.estado("chat_proprio") else {
        bail!("conversa própria não validada");
    };
    let Some(con) = app.conexao() else {
        bail!("sem conexão com o WhatsApp");
    };
    // Segundo número ligado e conectado: os avisos saem por ele, para o celular tocar.
    // Se ele falhar, o aviso vai pelo caminho normal (nada se perde).
    if app.segundo_ligado()
        && let Some(seg) = app.segundo()
    {
        let mut ok = true;
        for parte in dividir(texto, 3500) {
            match seg.enviar_ao_dono(&con, &parte).await {
                Ok(id) => app.banco.registrar_enviada(&id, agora())?,
                Err(e) => {
                    warn!("{e:#} — enviando pelo caminho normal");
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            info!("Aviso enviado pelo segundo número");
            return Ok(());
        }
    }
    // Divide mensagens muito longas.
    for parte in dividir(texto, 3500) {
        let id = con
            .enviar_ao_usuario(&destino, app.grupo_escolhido().as_deref(), &parte)
            .await?;
        app.banco.registrar_enviada(&id, agora())?;
    }
    info!("Aviso enviado para a própria conversa");
    Ok(())
}

fn adiar(app: &App, texto: &str) {
    let mut fila: Vec<String> = app
        .banco
        .estado("alertas_adiados")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    fila.push(texto.to_string());
    if let Ok(s) = serde_json::to_string(&fila) {
        let _ = app.banco.definir_estado("alertas_adiados", &s);
    }
}

/// Envia os avisos guardados durante o silêncio (se o silêncio acabou).
pub async fn liberar_adiados(app: &App) {
    if em_silencio(&app.cfg.ia.silencio) {
        return;
    }
    let Some(s) = app.banco.estado("alertas_adiados") else { return };
    let fila: Vec<String> = serde_json::from_str(&s).unwrap_or_default();
    if fila.is_empty() {
        return;
    }
    let texto = format!("🌅 Avisos guardados durante o horário de silêncio:\n\n{}", fila.join("\n\n———\n\n"));
    let na_tela = mostrar_na_tela(app, &format!("🌅 {} aviso(s) guardados durante o silêncio\n{}", fila.len(), fila[0])).await;
    let r = if app.envio_habilitado() { enviar(app, &texto).await } else { Ok(()) };
    match r.or_else(|e| if na_tela { Ok(()) } else { Err(e) }) {
        Ok(()) => {
            let _ = app.banco.remover_estado("alertas_adiados");
        }
        Err(e) => warn!("Não foi possível liberar avisos adiados: {e}"),
    }
}

/// "23:00-07:00" → verdadeiro se agora estiver dentro do intervalo.
pub fn em_silencio(intervalo: &str) -> bool {
    em_silencio_em(intervalo, Local::now().time())
}

fn em_silencio_em(intervalo: &str, agora: NaiveTime) -> bool {
    let Some((a, b)) = intervalo.split_once('-') else { return false };
    let (Ok(ini), Ok(fim)) = (
        NaiveTime::parse_from_str(a.trim(), "%H:%M"),
        NaiveTime::parse_from_str(b.trim(), "%H:%M"),
    ) else {
        return false;
    };
    if ini <= fim {
        agora >= ini && agora < fim
    } else {
        agora >= ini || agora < fim
    }
}

fn dividir(texto: &str, max: usize) -> Vec<String> {
    let mut partes = Vec::new();
    let mut atual = String::new();
    for linha in texto.lines() {
        if atual.chars().count() + linha.chars().count() + 1 > max && !atual.is_empty() {
            partes.push(std::mem::take(&mut atual));
        }
        if !atual.is_empty() {
            atual.push('\n');
        }
        atual.push_str(linha);
    }
    if !atual.is_empty() {
        partes.push(atual);
    }
    partes
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn silencio_atravessando_meia_noite() {
        let h = |s| NaiveTime::parse_from_str(s, "%H:%M").unwrap();
        assert!(em_silencio_em("23:00-07:00", h("23:30")));
        assert!(em_silencio_em("23:00-07:00", h("03:00")));
        assert!(!em_silencio_em("23:00-07:00", h("12:00")));
        assert!(em_silencio_em("13:00-14:00", h("13:30")));
        assert!(!em_silencio_em("", h("13:30")));
    }

    #[test]
    fn texto_curto_para_a_notificacao() {
        let (t, c) = texto_da_notificacao("⏰ *Lembrete*: [3] Pagar\nPrazo: amanhã");
        assert_eq!(t, "⏰ Lembrete: [3] Pagar");
        assert_eq!(c, "Prazo: amanhã");
        let (t, c) = texto_da_notificacao("só uma linha");
        assert_eq!((t.as_str(), c.as_str()), ("Recado", "só uma linha"));
    }

    #[test]
    fn divide_textos_longos() {
        let t = "a\n".repeat(10);
        assert_eq!(dividir(&t, 5).len(), 4);
    }
}
