//! Painel local mínimo (http://127.0.0.1:PORTA). Só aceita acessos deste computador.

use crate::app::{AppRef, data_curta};
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use tracing::{info, warn};

const PAGINA: &str = include_str!("painel.html");

pub async fn rodar(app: AppRef) {
    let porta = app.cfg.porta_painel;
    let rotas = Router::new()
        .route("/", get(|| async { Html(PAGINA) }))
        .route("/api/estado", get(estado))
        .route("/api/conversas", get(conversas))
        .route("/api/conversas/monitorar", post(monitorar))
        .route("/api/conversas/monitorar-varias", post(monitorar_varias))
        .route("/api/conversas/buscar-antigas", post(buscar_antigas))
        .route("/api/conversas/buscar-tudo", post(buscar_tudo))
        .route("/api/conversas/parar-busca", post(parar_busca))
        .route("/api/conversas/busca", get(situacao_busca))
        .route("/api/conversas/atualizar-nomes", post(atualizar_nomes))
        .route("/api/mensagens", get(mensagens))
        .route("/api/pendencias", get(pendencias))
        .route("/api/pendencias/excluir", post(excluir_pendencias))
        .route("/api/notificacoes", post(ligar_notificacoes))
        .route("/api/notificacoes/testar", post(testar_notificacao))
        .route("/api/pausar", post(pausar))
        .route("/api/desconectar", post(desconectar))
        .route("/api/apagar-historico", post(apagar_historico))
        .route("/api/grupos", get(grupos))
        .route("/api/conversa-assistente", post(conversa_assistente))
        .route("/api/iniciar-com-windows", post(iniciar_com_windows))
        .route("/api/fechar", post(fechar))
        .route("/api/segundo-numero", post(segundo_numero))
        .layer(middleware::from_fn_with_state(porta, protecao))
        .with_state(app);
    let ender = format!("127.0.0.1:{porta}");
    match tokio::net::TcpListener::bind(&ender).await {
        Ok(l) => {
            info!("Painel em http://{ender}");
            if let Err(e) = axum::serve(l, rotas).await {
                warn!("Painel parou: {e}");
            }
        }
        Err(e) => warn!("Não foi possível abrir o painel na porta {porta}: {e}"),
    }
}

/// Bloqueia acessos vindos de sites (DNS rebinding / CSRF):
/// - o cabeçalho Host precisa ser 127.0.0.1 ou localhost;
/// - ações (POST) exigem o cabeçalho X-Assistente, que sites não conseguem enviar sem permissão.
async fn protecao(State(porta): State<u16>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let permitido = [format!("127.0.0.1:{porta}"), format!("localhost:{porta}")];
    if !permitido.iter().any(|p| p == host) {
        return (StatusCode::FORBIDDEN, "acesso negado").into_response();
    }
    if req.method() == axum::http::Method::POST && req.headers().get("x-assistente").is_none() {
        return (StatusCode::FORBIDDEN, "acesso negado").into_response();
    }
    let mut r = next.run(req).await;
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    h.insert("x-frame-options", "DENY".parse().unwrap());
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; img-src 'self' data:"
            .parse()
            .unwrap(),
    );
    r
}

fn qr_svg(codigo: &str) -> Option<String> {
    let qr = qrcode::QrCode::new(codigo.as_bytes()).ok()?;
    Some(
        qr.render::<qrcode::render::svg::Color>()
            .min_dimensions(260, 260)
            .quiet_zone(true)
            .build(),
    )
}

async fn estado(State(app): State<AppRef>) -> Json<Value> {
    let e = app.ler_estado();
    let c = app.banco.contadores();
    let s = app.ler_segundo();
    let segundo = json!({ "ligado": app.segundo_ligado(), "estado": s, "qr_svg": s.qr.as_deref().and_then(qr_svg) });
    Json(json!({
        "conexao": e,
        "qr_svg": e.qr.as_deref().and_then(qr_svg),
        "contadores": c,
        "historico_periodo": match (c.historico_de, c.historico_ate) {
            (Some(a), Some(b)) => format!("{} até {}", data_curta(a), data_curta(b)),
            _ => String::new(),
        },
        "pausado": app.pausado(),
        "envio_habilitado": app.envio_habilitado(),
        "conversa_assistente": match app.grupo_escolhido() {
            Some(g) => json!({ "tipo": "grupo", "jid": g, "nome": app.banco.nome_conversa(&g) }),
            None => json!({ "tipo": "propria", "jid": "propria", "nome": "Conversa comigo mesmo" }),
        },
        "grupo_configurado": app.cfg.grupo_assistente,
        "erro_ativacao": app.banco.estado("erro_ativacao"),
        "iniciar_com_windows": crate::sistema::iniciar_com_windows(),
        "notificar_windows": crate::notificacao::na_tela_ligada(&app),
        "computador": crate::computador::descrever(&app.computador),
        "ajuste_automatico": app.ajuste.is_some(),
        "ia_pensar": app.cfg.ia.pensar,
        "ia_baixando": crate::ollama::situacao_download(),
        "segundo": segundo,
        "ia_disponivel": app.ia_ok(),
        "ia_modelo": app.cfg.ia.modelo,
        "transcricao_disponivel": app.transcricao_disponivel.load(Ordering::Relaxed),
        "prefixo": app.cfg.prefixo_comando,
        "pasta_dados": app.pasta.to_string_lossy(),
    }))
}

#[derive(Deserialize)]
struct Filtro {
    #[serde(default)]
    q: String,
    chat: Option<String>,
}

async fn conversas(State(app): State<AppRef>, Query(f): Query<Filtro>) -> Response {
    match app.banco.listar_conversas(f.q.trim()) {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct Monitorar {
    jid: String,
    monitorada: bool,
    #[serde(default)]
    prioridade: i64,
}

async fn monitorar(State(app): State<AppRef>, Json(m): Json<Monitorar>) -> StatusCode {
    match app.banco.definir_monitoramento(&m.jid, m.monitorada, m.prioridade.clamp(0, 1)) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Deserialize)]
struct MonitorarVarias {
    jids: Vec<String>,
    monitorada: bool,
}

async fn monitorar_varias(State(app): State<AppRef>, Json(m): Json<MonitorarVarias>) -> StatusCode {
    let c = app.banco.conexao();
    for jid in &m.jids {
        if c.execute("UPDATE conversas SET monitorada=?2 WHERE jid=?1", rusqlite::params![jid, m.monitorada]).is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct BuscarAntigas {
    jid: String,
    /// Quantas mensagens pedir de uma vez (padrão 50).
    #[serde(default)]
    quantidade: Option<i32>,
}

/// Pede ao celular as 50 mensagens anteriores à mais antiga guardada desta conversa.
async fn buscar_antigas(State(app): State<AppRef>, Json(b): Json<BuscarAntigas>) -> Response {
    let erro = |t: &str| (StatusCode::BAD_REQUEST, t.to_string()).into_response();
    let Some(con) = app.conexao() else { return erro("O assistente não está conectado ao WhatsApp.") };
    if !app.ler_estado().conectado {
        return erro("O assistente não está conectado ao WhatsApp.");
    }
    if !app.banco.conversa_monitorada(&b.jid) {
        return erro("Esta conversa está com Ler desmarcado.");
    }
    let Some((id, de_mim, data)) = app.banco.mensagem_mais_antiga(&b.jid) else {
        return erro("Ainda não há nenhuma mensagem desta conversa para servir de ponto de partida.");
    };
    let qtd = b.quantidade.unwrap_or(50).clamp(1, 1000);
    match con.pedir_mensagens_antigas(&b.jid, &id, de_mim, data, qtd).await {
        Ok(()) => {
            info!("Pedido de mensagens antigas enviado ao celular");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            warn!("{e}");
            erro(&e.to_string())
        }
    }
}

/// Situação da busca completa (uma de cada vez), guardada em `estado`.
fn gravar_busca(app: &AppRef, v: Value) {
    let _ = app.banco.definir_estado("busca_antigas", &v.to_string());
}

/// Procura de novo os nomes da agenda e os números dos contatos ocultos.
async fn atualizar_nomes(State(app): State<AppRef>) -> Response {
    let Some(con) = app.conexao() else {
        return (StatusCode::BAD_REQUEST, "O assistente não está conectado ao WhatsApp.").into_response();
    };
    crate::whatsapp::atualizar_nomes(&app, &con, false).await;
    StatusCode::NO_CONTENT.into_response()
}

async fn situacao_busca(State(app): State<AppRef>) -> Json<Value> {
    Json(
        app.banco
            .estado("busca_antigas")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(json!({ "ativa": false })),
    )
}

async fn parar_busca(State(app): State<AppRef>) -> StatusCode {
    let _ = app.banco.definir_estado("busca_antigas_parar", "1");
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct BuscarTudo {
    /// Uma conversa só...
    #[serde(default)]
    jid: Option<String>,
    /// ...ou todas as conversas com "Ler" marcado.
    #[serde(default)]
    todas: bool,
}

/// Busca o histórico antigo, conversa por conversa, repetindo o pedido de 50
/// mensagens até o celular parar de mandar (chegou ao começo da conversa).
/// O celular não manda mais que ~50 por pedido; o ganho de velocidade vem de
/// pedir de novo assim que a resposta chega (pausa curta entre pedidos).
async fn buscar_tudo(State(app): State<AppRef>, Json(b): Json<BuscarTudo>) -> Response {
    let erro = |t: &str| (StatusCode::BAD_REQUEST, t.to_string()).into_response();
    let ativa = app
        .banco
        .estado("busca_antigas")
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some_and(|v| v["ativa"] == json!(true));
    if ativa {
        return erro("Já há uma busca em andamento. Espere terminar ou clique em Parar.");
    }
    if app.conexao().is_none() || !app.ler_estado().conectado {
        return erro("O assistente não está conectado ao WhatsApp.");
    }
    // Fila: as conversas mais movimentadas primeiro.
    let fila: Vec<(String, String)> = if b.todas {
        let mut l = app.banco.listar_conversas("").unwrap_or_default();
        l.retain(|c| c.monitorada && c.total_msgs > 0 && !ja_completa(&app, &c.jid));
        l.sort_by_key(|c| std::cmp::Reverse(c.total_msgs));
        l.into_iter().map(|c| (c.jid, c.nome)).collect()
    } else {
        let Some(jid) = b.jid else { return erro("Diga qual conversa.") };
        if !app.banco.conversa_monitorada(&jid) {
            return erro("Esta conversa está com Ler desmarcado.");
        }
        if app.banco.mensagem_mais_antiga(&jid).is_none() {
            return erro("Ainda não há nenhuma mensagem desta conversa para servir de ponto de partida.");
        }
        let nome = app.banco.nome_conversa(&jid);
        vec![(jid, nome)]
    };
    if fila.is_empty() {
        return erro("Nada a buscar: as conversas com Ler marcado já foram buscadas até o começo (ou não têm mensagens).");
    }
    let _ = app.banco.remover_estado("busca_antigas_parar");
    let total_conversas = fila.len();
    gravar_busca(&app, json!({ "ativa": true, "nome": fila[0].1, "texto": "Começando…" }));

    let app2 = app.clone();
    tokio::spawn(async move {
        let mut trazidas_total = 0i64;
        let mut fim = String::new();
        for (n, (jid, nome)) in fila.iter().enumerate() {
            let posicao = if total_conversas > 1 {
                format!("Conversa {} de {total_conversas}. ", n + 1)
            } else {
                String::new()
            };
            let (motivo, trazidas) = buscar_conversa(&app2, jid, nome, &posicao, trazidas_total).await;
            trazidas_total += trazidas;
            if motivo != Motivo::Comeco {
                fim = match motivo {
                    Motivo::Parado => "Parado por você.".into(),
                    Motivo::Conexao => "Conexão perdida. Tente de novo depois.".into(),
                    Motivo::Erro(e) => format!("Parou: {e}"),
                    Motivo::Comeco => unreachable!(),
                };
                break;
            }
        }
        if fim.is_empty() {
            fim = if total_conversas > 1 {
                format!("Terminou as {total_conversas} conversas.")
            } else {
                let desde = app2.banco.mensagem_mais_antiga(&fila[0].0).map(|m| data_com_ano(m.2)).unwrap_or_default();
                format!("Terminou: o celular não tem mensagens mais antigas. Guardado desde {desde}.")
            };
        }
        info!("Busca de mensagens antigas terminou ({trazidas_total} mensagens)");
        gravar_busca(&app2, json!({
            "ativa": false, "nome": if total_conversas > 1 { "Todas as conversas".to_string() } else { fila[0].1.clone() },
            "novas": trazidas_total,
            "texto": format!("{fim} Trazidas: {trazidas_total} mensagens antigas."),
        }));
    });
    StatusCode::NO_CONTENT.into_response()
}

/// Guarda que esta conversa já foi buscada até o começo (com a data mais antiga daquele momento).
fn marcar_completa(app: &AppRef, jid: &str) {
    if let Some(m) = app.banco.mensagem_mais_antiga(jid) {
        let _ = app.banco.definir_estado(&format!("historico_completo:{jid}"), &m.2.to_string());
    }
}

fn ja_completa(app: &AppRef, jid: &str) -> bool {
    let marca = app.banco.estado(&format!("historico_completo:{jid}")).and_then(|s| s.parse::<i64>().ok());
    marca.is_some() && app.banco.mensagem_mais_antiga(jid).map(|m| m.2) == marca
}

#[derive(PartialEq)]
enum Motivo {
    /// O celular parou de mandar: chegou ao começo (ou ao limite que ele guarda).
    Comeco,
    Parado,
    Conexao,
    Erro(String),
}

/// Busca uma conversa até o começo. Devolve o motivo do fim e quantas mensagens trouxe.
async fn buscar_conversa(app: &AppRef, jid: &str, nome: &str, posicao: &str, antes: i64) -> (Motivo, i64) {
    use std::time::Duration;
    let inicial = contar(app, jid);
    let mut sem_resposta = 0;
    loop {
        if app.banco.estado("busca_antigas_parar").is_some() {
            return (Motivo::Parado, contar(app, jid) - inicial);
        }
        let (Some(con), Some((id, de_mim, data))) = (app.conexao(), app.banco.mensagem_mais_antiga(jid)) else {
            return (Motivo::Conexao, contar(app, jid) - inicial);
        };
        let pedido_em = crate::app::agora_ms();
        if let Err(e) = con.pedir_mensagens_antigas(jid, &id, de_mim, data, 50).await {
            return (Motivo::Erro(e.to_string()), contar(app, jid) - inicial);
        }
        // Espera a resposta (normalmente 1–3 s), conferindo 4 vezes por segundo.
        // Se chegar um pacote sem nada mais antigo, a conversa acabou.
        let mut chegou = false;
        let mut resposta_vazia = false;
        for _ in 0..(4 * 12) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if app.banco.mensagem_mais_antiga(jid).is_some_and(|m| m.2 < data) {
                chegou = true;
                break;
            }
            if app.historico_recebido.load(std::sync::atomic::Ordering::Relaxed) > pedido_em + 300 {
                // Dá um instante para a gravação terminar antes de concluir.
                tokio::time::sleep(Duration::from_millis(500)).await;
                chegou = app.banco.mensagem_mais_antiga(jid).is_some_and(|m| m.2 < data);
                resposta_vazia = !chegou;
                break;
            }
        }
        let trazidas = contar(app, jid) - inicial;
        if resposta_vazia {
            marcar_completa(app, jid);
            return (Motivo::Comeco, trazidas);
        }
        if !chegou {
            // Uma segunda chance antes de concluir que acabou.
            sem_resposta += 1;
            if sem_resposta >= 2 {
                marcar_completa(app, jid);
                return (Motivo::Comeco, trazidas);
            }
            continue;
        }
        sem_resposta = 0;
        let desde = app.banco.mensagem_mais_antiga(jid).map(|m| data_com_ano(m.2)).unwrap_or_default();
        gravar_busca(app, json!({
            "ativa": true, "nome": nome, "novas": antes + trazidas,
            "texto": format!("{posicao}Buscando… {trazidas} mensagens antigas desta conversa, já desde {desde}. Total trazido: {}.", antes + trazidas),
        }));
        // Pausa curta para o celular respirar e a gravação terminar.
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
}

fn data_com_ano(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%d/%m/%Y").to_string())
        .unwrap_or_default()
}

fn contar(app: &AppRef, jid: &str) -> i64 {
    app.banco
        .conexao()
        .query_row("SELECT COUNT(*) FROM mensagens WHERE chat=?1", [jid], |r| r.get(0))
        .unwrap_or(0)
}

async fn mensagens(State(app): State<AppRef>, Query(f): Query<Filtro>) -> Response {
    let r = if f.q.trim().is_empty() {
        app.banco.ultimas_mensagens(f.chat.as_deref(), 100)
    } else {
        app.banco.buscar(f.q.trim(), 100)
    };
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn pendencias(State(app): State<AppRef>) -> Response {
    let r: anyhow::Result<Vec<Value>> = (|| {
        let c = app.banco.conexao();
        let mut st = c.prepare(
            "SELECT i.id, i.tipo, i.descricao, i.prazo_texto, i.prazo, i.importancia, i.estado, i.chat, i.criado
             FROM itens i ORDER BY (i.estado='pendente') DESC, i.criado DESC LIMIT 200",
        )?;
        let v = st
            .query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?, "tipo": r.get::<_, String>(1)?, "descricao": r.get::<_, String>(2)?,
                    "prazo_texto": r.get::<_, Option<String>>(3)?, "prazo": r.get::<_, Option<i64>>(4)?,
                    "importancia": r.get::<_, String>(5)?, "estado": r.get::<_, String>(6)?,
                    "chat": r.get::<_, String>(7)?, "criado": r.get::<_, i64>(8)?,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    })();
    match r {
        Ok(mut v) => {
            for item in &mut v {
                let chat = item["chat"].as_str().unwrap_or("").to_string();
                item["nome_chat"] = json!(app.banco.nome_conversa(&chat));
            }
            Json(v).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct Pausar {
    pausado: bool,
}

async fn pausar(State(app): State<AppRef>, Json(p): Json<Pausar>) -> StatusCode {
    app.definir_pausado(p.pausado);
    StatusCode::NO_CONTENT
}

/// Desconecta este computador do WhatsApp e apaga a sessão local.
async fn desconectar(State(app): State<AppRef>, _h: HeaderMap) -> StatusCode {
    if let Some(con) = app.conexao() {
        con.sair().await;
    }
    let _ = app.banco.remover_estado("envio_habilitado");
    let _ = app.banco.remover_estado("chat_proprio");
    // A sessão é apagada na próxima abertura (o arquivo está em uso agora).
    let _ = std::fs::write(app.pasta.join("apagar-sessao.marca"), b"1");
    app.alterar_estado(|e| {
        e.conectado = false;
        e.logado = false;
        e.desconectado_pelo_usuario = true;
        e.ultimo_evento = "Desconectado. Feche e abra o assistente para conectar de novo.".into();
    });
    info!("Usuário desconectou a sessão pelo painel");
    StatusCode::NO_CONTENT
}

/// Grupos conhecidos (para escolher a conversa do assistente).
async fn grupos(State(app): State<AppRef>) -> Response {
    match app.banco.listar_conversas("") {
        Ok(v) => {
            let mut g: Vec<Value> = v
                .into_iter()
                .filter(|c| c.grupo)
                .map(|c| json!({ "jid": c.jid, "nome": c.nome }))
                .collect();
            g.sort_by_key(|x| x["nome"].as_str().unwrap_or("").to_lowercase());
            Json(g).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct Escolha {
    valor: String,
}

/// Escolhe onde o assistente conversa: "propria" ou o JID de um grupo.
async fn conversa_assistente(State(app): State<AppRef>, Json(e): Json<Escolha>) -> StatusCode {
    let v = e.valor.trim();
    let valido = v == "propria"
        || (v.ends_with("@g.us") && app.banco.listar_conversas("").is_ok_and(|l| l.iter().any(|c| c.grupo && c.jid == v)));
    if !valido {
        return StatusCode::BAD_REQUEST;
    }
    app.escolher_conversa(v);
    info!("Conversa do assistente alterada pelo painel");
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct Ligar {
    ativar: bool,
}

async fn iniciar_com_windows(Json(l): Json<Ligar>) -> StatusCode {
    match crate::sistema::definir_iniciar_com_windows(l.ativar) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(e) => {
            warn!("Não foi possível mudar 'iniciar com o Windows': {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[derive(Deserialize)]
struct Excluir {
    id: Option<i64>,
    #[serde(default)]
    todas: bool,
}

/// Exclui uma pendência (pelo número) ou todas.
async fn excluir_pendencias(State(app): State<AppRef>, Json(e): Json<Excluir>) -> Response {
    let c = app.banco.conexao();
    let r = match (e.todas, e.id) {
        (true, _) => c.execute("DELETE FROM itens", []),
        (false, Some(id)) => c.execute("DELETE FROM itens WHERE id=?1", [id]),
        _ => return (StatusCode::BAD_REQUEST, "diga qual pendência").into_response(),
    };
    match r {
        Ok(n) => {
            info!("{n} pendência(s) excluída(s) pelo painel");
            Json(json!({ "excluidas": n })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn ligar_notificacoes(State(app): State<AppRef>, Json(l): Json<Ligar>) -> StatusCode {
    match app.banco.definir_estado("notificar_windows", if l.ativar { "1" } else { "0" }) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn testar_notificacao(State(app): State<AppRef>) -> Response {
    let pasta = app.pasta.clone();
    let url = format!("http://127.0.0.1:{}", app.cfg.porta_painel);
    let r = tokio::task::spawn_blocking(move || {
        crate::sistema::notificar(&pasta, "Teste do assistente", "As notificações do Windows estão funcionando. Clique aqui para abrir o painel.", &url)
    })
    .await;
    match r {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Liga ou desliga o segundo número. Desligar também o desvincula e apaga a sessão dele.
async fn segundo_numero(State(app): State<AppRef>, Json(l): Json<Ligar>) -> StatusCode {
    if !l.ativar && let Some(seg) = app.segundo() {
        seg.sair().await;
    }
    if !l.ativar {
        let _ = app.banco.remover_estado("segundo_arquivo");
        let _ = app.banco.remover_estado("segundo_pn");
        let _ = app.banco.remover_estado("segundo_lid");
    }
    let _ = app.banco.definir_estado("segundo_numero", if l.ativar { "1" } else { "0" });
    app.alterar_segundo(|e| e.ultimo_evento.clear());
    app.mudou_segundo.notify_one();
    info!("Segundo número {} pelo painel", if l.ativar { "ligado" } else { "desligado" });
    StatusCode::NO_CONTENT
}

async fn fechar(State(app): State<AppRef>) -> StatusCode {
    app.fechar();
    StatusCode::NO_CONTENT
}

async fn apagar_historico(State(app): State<AppRef>) -> StatusCode {
    match app.banco.apagar_historico() {
        Ok(()) => {
            info!("Histórico apagado pelo usuário");
            StatusCode::NO_CONTENT
        }
        Err(e) => {
            warn!("Falha ao apagar histórico: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
