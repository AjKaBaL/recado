//! Recado — assistente pessoal para WhatsApp. Ponto de entrada.

// Na versão final não abre janela preta: o assistente fica no ícone perto do relógio.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agendador;
mod analise;
mod app;
mod autoteste;
mod banco;
mod comandos;
mod computador;
mod config;
mod ia;
mod notificacao;
mod ollama;
mod painel;
mod resumos;
mod sistema;
mod transcricao;
mod whatsapp;

use anyhow::{Context, Result};
use app::{App, EstadoConexao};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriterExt;

fn iniciar_logs(pasta: &std::path::Path) -> Result<()> {
    let dir = pasta.join("logs");
    std::fs::create_dir_all(&dir)?;
    // Mantém só os 7 arquivos de log mais recentes.
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut arqs: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        arqs.sort();
        while arqs.len() > 7 {
            let _ = std::fs::remove_file(arqs.remove(0));
        }
    }
    let arq = dir.join(format!("assistente-{}.log", chrono::Local::now().format("%Y-%m-%d")));
    let f = std::fs::OpenOptions::new().create(true).append(true).open(arq)?;
    let filtro = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,whatsapp_rust=warn,wacore=warn,diesel=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filtro)
        .with_ansi(false)
        .with_target(false)
        .with_writer(std::io::stdout.and(std::sync::Mutex::new(f)))
        .init();
    Ok(())
}

use sistema::abrir_navegador;

fn apagar_sessao(pasta: &std::path::Path) {
    for nome in ["sessao-whatsapp.db", "sessao-whatsapp.db-wal", "sessao-whatsapp.db-shm"] {
        let _ = std::fs::remove_file(pasta.join(nome));
    }
    let _ = std::fs::remove_file(pasta.join("apagar-sessao.marca"));
}

#[tokio::main]
async fn main() {
    if let Err(e) = principal().await {
        error!("Erro fatal: {e:#}");
        sistema::janela_de_erro(&format!("O assistente não conseguiu iniciar:\n\n{e:#}"));
    }
    // Sai na hora, sem esperar tarefas de fundo (conexão, análise) terminarem sozinhas.
    // O banco grava cada mudança por completo, então nada se perde.
    std::process::exit(0);
}

async fn principal() -> Result<()> {
    let pasta = config::pasta_dados();
    std::fs::create_dir_all(&pasta).context("criando pasta de dados")?;
    iniciar_logs(&pasta)?;
    let mut cfg = config::Config::carregar(&pasta)?;
    // Descobre como é este computador e escolhe as opções da IA e da transcrição.
    let computador = computador::detectar();
    let ajuste = cfg.ajuste_automatico.then(|| computador::recomendar(&computador));
    if let Some(a) = &ajuste {
        computador::aplicar(&mut cfg, a);
    }
    let url_painel = format!("http://127.0.0.1:{}", cfg.porta_painel);

    let args: Vec<String> = std::env::args().collect();
    let segundo_plano = args.iter().any(|a| a == sistema::ARG_SEGUNDO_PLANO);
    if args.get(1).is_some_and(|a| a.starts_with("--testar") || a == "--perguntar") {
        // Autotestes: mostram o resultado numa janela de texto.
        let nova_janela = sistema::abrir_console();
        let (pasta_t, cfg_t) = (pasta.clone(), cfg.clone());
        let r = tokio::task::spawn_blocking(move || autoteste::executar(&args, &pasta_t, cfg_t))
            .await?
            .unwrap_or(Ok(()));
        if let Err(e) = &r {
            eprintln!("\nERRO: {e:#}");
        }
        if nova_janela {
            eprintln!("\nPressione Enter para fechar esta janela...");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        return Ok(());
    }

    // Só uma cópia do assistente por vez: se já estiver aberto, só mostra o painel.
    if std::net::TcpListener::bind(("127.0.0.1", cfg.porta_painel)).is_err() {
        if !segundo_plano {
            abrir_navegador(&url_painel);
        }
        return Ok(());
    }
    info!("Iniciando assistente");
    sistema::migrar_nome_antigo();
    info!(
        "Computador: {} → IA {}",
        computador::descrever(&computador),
        if ajuste.is_some() { format!("{} (ajuste automático)", cfg.ia.modelo) } else { format!("{} (manual)", cfg.ia.modelo) }
    );

    if pasta.join("apagar-sessao.marca").exists() {
        apagar_sessao(&pasta);
        info!("Sessão anterior apagada a pedido do usuário");
    }

    let banco = banco::Banco::abrir(&pasta.join("assistente.db"))?;
    // Uma busca de mensagens antigas interrompida pelo fechamento não continua sozinha.
    let _ = banco.remover_estado("busca_antigas");
    let (tx_audio, rx_audio) = mpsc::channel(500);
    let app = Arc::new(App {
        pasta: pasta.clone(),
        cfg: cfg.clone(),
        banco,
        estado: RwLock::new(EstadoConexao {
            ultimo_evento: "Iniciando conexão…".into(),
            ..Default::default()
        }),
        conexao: RwLock::new(None),
        fila_audio: tx_audio,
        ia_disponivel: AtomicBool::new(false),
        transcricao_disponivel: AtomicBool::new(false),
        sair: tokio::sync::Notify::new(),
        pedido_sair: AtomicBool::new(false),
        bandeja_ativa: AtomicBool::new(false),
        historico_recebido: std::sync::atomic::AtomicI64::new(0),
        segundo: RwLock::new(None),
        estado_segundo: RwLock::new(Default::default()),
        mudou_segundo: tokio::sync::Notify::new(),
        computador: computador.clone(),
        ajuste: ajuste.clone(),
    });

    tokio::spawn(painel::rodar(app.clone()));
    tokio::spawn(transcricao::rodar(app.clone(), rx_audio));
    // Abre a IA local (Ollama) junto com o assistente, se ainda não estiver aberta.
    {
        let a = app.clone();
        tokio::task::spawn_blocking(move || ollama::garantir(&a, Duration::from_secs(120)));
    }
    tokio::spawn(agendador::rodar(app.clone()));
    tokio::spawn(whatsapp::rodar_segundo_numero(app.clone()));
    sistema::iniciar_bandeja(app.clone(), url_painel.clone());
    if cfg.abrir_navegador && !segundo_plano {
        abrir_navegador(&url_painel);
    }

    let conexao = async {
        loop {
            match whatsapp::construir(app.clone()).await {
                Ok(bot) => bot.run().await,
                Err(e) => warn!("{e:#}"),
            }
            // A conexão terminou (desconectado pelo usuário, sessão encerrada ou erro).
            if let Ok(mut g) = app.conexao.write() {
                *g = None;
            }
            if pasta.join("apagar-sessao.marca").exists() {
                apagar_sessao(&pasta);
                info!("Sessão apagada; um novo QR Code será gerado");
            }
            app.alterar_estado(|e| {
                e.conectado = false;
                if !e.desconectado_pelo_usuario {
                    e.ultimo_evento = "Conexão encerrada — tentando de novo em instantes…".into();
                }
                e.desconectado_pelo_usuario = false;
            });
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    };

    tokio::select! {
        _ = conexao => {}
        _ = tokio::signal::ctrl_c() => {
            info!("Encerrando a pedido do usuário");
        }
        _ = app.sair.notified() => {
            info!("Encerrando a pedido do usuário");
        }
    }
    // Fecha a IA local junto.
    let a = app.clone();
    let _ = tokio::task::spawn_blocking(move || ollama::encerrar(&a)).await;
    Ok(())
}
