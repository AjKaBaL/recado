//! Abre e fecha o Ollama (a IA local) junto com o assistente.
//!
//! - Ao iniciar, e sempre que a IA for chamada e não responder, o assistente abre o
//!   Ollama sozinho (sem janela) e espera ele ficar pronto.
//! - Ao fechar o assistente, fecha também o Ollama (opção `ia.fechar_ollama_junto`).

use crate::app::App;
use crate::ia::ProvedorIa;
use crate::resumos;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// O Ollama que o próprio assistente abriu (para fechar depois).
static ABERTO_POR_NOS: Mutex<Option<Child>> = Mutex::new(None);
/// Evita duas tentativas de abrir ao mesmo tempo.
static ABRINDO: Mutex<()> = Mutex::new(());
/// Andamento do download do modelo (None = nenhum download).
static BAIXANDO: Mutex<Option<String>> = Mutex::new(None);

pub fn situacao_download() -> Option<String> {
    BAIXANDO.lock().ok().and_then(|g| g.clone())
}

/// O Ollama está aberto (mesmo que o modelo ainda não exista)?
fn servidor_no_ar(url: &str) -> bool {
    ureq::get(format!("{}/api/tags", url.trim_end_matches('/')))
        .config()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .call()
        .is_ok()
}

/// O Ollama está aberto mas o modelo escolhido não está instalado: baixa
/// (uma vez, em segundo plano). O andamento aparece no painel.
fn baixar_modelo(app: &App) {
    {
        let Ok(mut g) = BAIXANDO.lock() else { return };
        if g.is_some() {
            return;
        }
        *g = Some("começando…".into());
    }
    let url = app.cfg.ia.url_ollama.trim_end_matches('/').to_string();
    let modelo = app.cfg.ia.modelo.clone();
    info!("Baixando o modelo da IA ({modelo}) pelo Ollama");
    std::thread::spawn(move || {
        let r = (|| -> anyhow::Result<()> {
            use std::io::BufRead;
            let agente: ureq::Agent = ureq::Agent::config_builder().timeout_global(None).build().into();
            let mut resp = agente
                .post(format!("{url}/api/pull"))
                .send_json(serde_json::json!({ "model": modelo, "stream": true }))?;
            let leitor = std::io::BufReader::new(resp.body_mut().as_reader());
            for linha in leitor.lines() {
                let v: serde_json::Value = serde_json::from_str(&linha?).unwrap_or_default();
                if let Some(e) = v["error"].as_str() {
                    anyhow::bail!("{e}");
                }
                let (feito, total) = (v["completed"].as_f64(), v["total"].as_f64());
                let txt = match (feito, total) {
                    (Some(f), Some(t)) if t > 0.0 => format!("{:.0}% de {:.1} GB", f / t * 100.0, t / 1e9),
                    _ => v["status"].as_str().unwrap_or("").to_string(),
                };
                if let Ok(mut g) = BAIXANDO.lock() {
                    *g = Some(txt);
                }
            }
            Ok(())
        })();
        match r {
            Ok(()) => info!("Modelo da IA baixado"),
            Err(e) => warn!("Não foi possível baixar o modelo da IA: {e}"),
        }
        if let Ok(mut g) = BAIXANDO.lock() {
            *g = None;
        }
    });
}

fn url_local(url: &str) -> bool {
    url.contains("127.0.0.1") || url.contains("localhost")
}

/// Onde está o ollama.exe: no PATH ou na pasta padrão de instalação.
fn localizar() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(l).join("Programs").join("Ollama"));
    }
    if let Ok(p) = std::env::var("ProgramFiles") {
        dirs.push(PathBuf::from(p).join("Ollama"));
    }
    dirs.into_iter().map(|d| d.join("ollama.exe")).find(|p| p.exists())
}

fn responde(app: &App) -> bool {
    resumos::provedor(app).disponivel().is_ok()
}

/// Garante que a IA está pronta: se não estiver, abre o Ollama e espera até
/// `espera` (o modelo também precisa existir). Bloqueia; use em `spawn_blocking`.
pub fn garantir(app: &App, espera: Duration) -> bool {
    if !app.cfg.ia.ativada {
        return false;
    }
    if responde(app) {
        app.ia_disponivel.store(true, Ordering::Relaxed);
        return true;
    }
    if servidor_no_ar(&app.cfg.ia.url_ollama) {
        // Aberto, mas sem o modelo: baixa (só no Ollama deste computador).
        app.ia_disponivel.store(false, Ordering::Relaxed);
        if url_local(&app.cfg.ia.url_ollama) {
            baixar_modelo(app);
        }
        return false;
    }
    if !url_local(&app.cfg.ia.url_ollama) {
        app.ia_disponivel.store(false, Ordering::Relaxed);
        return false;
    }
    let _vez = ABRINDO.lock().unwrap_or_else(|e| e.into_inner());
    // Outra tarefa pode ter aberto enquanto esperávamos a vez.
    if responde(app) {
        app.ia_disponivel.store(true, Ordering::Relaxed);
        return true;
    }
    let Some(exe) = localizar() else {
        warn!("Ollama não encontrado neste computador");
        return false;
    };
    let ja_rodando = ABERTO_POR_NOS
        .lock()
        .map(|mut g| g.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None))))
        .unwrap_or(false);
    if !ja_rodando {
        info!("Abrindo a IA local (Ollama)");
        let mut cmd = Command::new(&exe);
        cmd.arg("serve").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // sem janela
        }
        match cmd.spawn() {
            Ok(filho) => {
                if let Ok(mut g) = ABERTO_POR_NOS.lock() {
                    *g = Some(filho);
                }
            }
            Err(e) => {
                warn!("Não foi possível abrir o Ollama: {e}");
                return false;
            }
        }
    }
    let inicio = Instant::now();
    while inicio.elapsed() < espera {
        std::thread::sleep(Duration::from_secs(2));
        if responde(app) {
            app.ia_disponivel.store(true, Ordering::Relaxed);
            info!("IA local pronta ({} s)", inicio.elapsed().as_secs());
            return true;
        }
        if servidor_no_ar(&app.cfg.ia.url_ollama) {
            baixar_modelo(app);
            return false;
        }
    }
    warn!("O Ollama não ficou pronto a tempo");
    false
}

/// Fecha o Ollama ao sair do assistente.
pub fn encerrar(app: &App) {
    if let Ok(mut g) = ABERTO_POR_NOS.lock()
        && let Some(mut filho) = g.take()
    {
        let _ = filho.kill();
        let _ = filho.wait();
    }
    // O Ollama também pode ter sido aberto pelo Windows ou por você: fecha tudo,
    // como pedido, a não ser que a opção esteja desligada.
    #[cfg(windows)]
    if app.cfg.ia.fechar_ollama_junto && url_local(&app.cfg.ia.url_ollama) {
        use std::os::windows::process::CommandExt;
        for nome in ["ollama app.exe", "ollama.exe"] {
            let _ = Command::new("taskkill")
                .args(["/IM", nome, "/F", "/T"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000)
                .status();
        }
        info!("IA local (Ollama) fechada junto com o assistente");
    }
    #[cfg(not(windows))]
    let _ = app;
}
