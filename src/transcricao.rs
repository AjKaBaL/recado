//! Transcrição local de áudios (FFmpeg + whisper.cpp). Nada sai do computador.

use crate::app::{App, AppRef, TarefaAudio};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
use tracing::{info, warn};

#[cfg(windows)]
const SEM_JANELA: u32 = 0x0800_0000; // CREATE_NO_WINDOW

fn comando(exe: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(exe);
    // Não deixar o conteúdo dos áudios aparecer na tela nem em logs.
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(SEM_JANELA);
    }
    c
}

pub struct Ferramentas {
    pub ffmpeg: PathBuf,
    pub whisper: PathBuf,
    pub modelo: PathBuf,
}

/// Procura FFmpeg, whisper-cli e o modelo.
pub fn localizar(app: &App) -> Result<Ferramentas> {
    let pasta = app.pasta.join("ferramentas");
    let ffmpeg = which("ffmpeg.exe")
        .or_else(|| Some(pasta.join("ffmpeg.exe")).filter(|p| p.exists()))
        .context("FFmpeg não encontrado")?;
    let whisper = if app.cfg.transcricao.whisper_exe.is_empty() {
        ["whisper-cli.exe", "Release/whisper-cli.exe", "whisper/whisper-cli.exe", "whisper/Release/whisper-cli.exe"]
            .iter()
            .map(|n| pasta.join(n))
            .find(|p| p.exists())
            .context("whisper-cli.exe não encontrado na pasta ferramentas")?
    } else {
        PathBuf::from(&app.cfg.transcricao.whisper_exe)
    };
    let preferido = pasta.join(format!("ggml-{}.bin", app.cfg.transcricao.preferir_modelo));
    let modelo = if app.cfg.transcricao.modelo.is_empty() && !app.cfg.transcricao.preferir_modelo.is_empty() && preferido.exists() {
        preferido
    } else if app.cfg.transcricao.modelo.is_empty() {
        std::fs::read_dir(&pasta)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("ggml-") && n.ends_with(".bin")))
            .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
            .context("modelo ggml-*.bin não encontrado na pasta ferramentas")?
    } else {
        PathBuf::from(&app.cfg.transcricao.modelo)
    };
    if !whisper.exists() || !modelo.exists() {
        bail!("whisper ou modelo não encontrados");
    }
    Ok(Ferramentas { ffmpeg, whisper, modelo })
}

fn which(nome: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(l).join("Microsoft").join("WinGet").join("Links"));
    }
    dirs.into_iter().map(|d| d.join(nome)).find(|p| p.exists())
}

/// Transcreve um arquivo de áudio (qualquer formato que o FFmpeg leia).
pub fn transcrever(f: &Ferramentas, audio: &Path, threads: u32) -> Result<String> {
    let wav = audio.with_extension("wav");
    let saida = audio.with_extension("");
    let st = comando(&f.ffmpeg)
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(audio)
        .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"])
        .arg(&wav)
        .status()
        .context("não foi possível executar o FFmpeg")?;
    if !st.success() {
        bail!("FFmpeg não conseguiu converter o áudio");
    }
    let st = comando(&f.whisper)
        .arg("-m").arg(&f.modelo)
        .args(["-l", "pt", "-nt", "-np", "-otxt", "-t", &threads.max(1).to_string()])
        .arg("-of").arg(&saida)
        .arg("-f").arg(&wav)
        .status()
        .context("não foi possível executar o whisper")?;
    let _ = std::fs::remove_file(&wav);
    let txt_arq = saida.with_extension("txt");
    if !st.success() {
        bail!("whisper falhou ao transcrever");
    }
    let texto = std::fs::read_to_string(&txt_arq).context("transcrição não gerada")?;
    let _ = std::fs::remove_file(&txt_arq);
    let texto = texto.split_whitespace().collect::<Vec<_>>().join(" ");
    if texto.is_empty() {
        bail!("áudio sem fala reconhecível");
    }
    Ok(texto)
}

/// Trabalhador: um áudio por vez, para não sobrecarregar o computador.
pub async fn rodar(app: AppRef, mut fila: mpsc::Receiver<TarefaAudio>) {
    let a = app.clone();
    let ferramentas = tokio::task::spawn_blocking(move || localizar(&a)).await.ok().and_then(|r| match r {
        Ok(f) => Some(f),
        Err(e) => {
            warn!("Transcrição de áudio indisponível: {e}");
            None
        }
    });
    let mut ferramentas = ferramentas.map(std::sync::Arc::new);
    app.transcricao_disponivel.store(ferramentas.is_some(), Ordering::Relaxed);
    if ferramentas.is_some() {
        info!("Transcrição de áudio local pronta");
        reprocessar_falhas(&app);
        // Reprocessa áudios que ficaram pendentes (ex.: o programa foi fechado no meio).
        let pend: Vec<(String, String, String)> = {
            let c = app.banco.conexao();
            c.prepare("SELECT chat, id, arquivo FROM audios WHERE estado='pendente' AND arquivo IS NOT NULL")
                .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect())
                .unwrap_or_default()
        };
        for (chat, id, arq) in pend {
            processar(&app, ferramentas.clone(), TarefaAudio { chat, id, arquivo: arq.into() }).await;
        }
    }
    while let Some(t) = fila.recv().await {
        // Se na abertura as ferramentas não foram achadas (ex.: antivírus examinando
        // o arquivo), tenta de novo a cada áudio.
        if ferramentas.is_none() {
            let a = app.clone();
            if let Ok(Ok(f)) = tokio::task::spawn_blocking(move || localizar(&a)).await {
                info!("Transcrição de áudio local pronta");
                ferramentas = Some(std::sync::Arc::new(f));
                app.transcricao_disponivel.store(true, Ordering::Relaxed);
                reprocessar_falhas(&app);
            }
        }
        processar(&app, ferramentas.clone(), t).await;
    }
}

/// Áudios que falharam só porque a transcrição não estava pronta voltam para a fila
/// (o arquivo continua guardado em `audios/`).
fn reprocessar_falhas(app: &AppRef) {
    let falhas: Vec<(String, String)> = {
        let c = app.banco.conexao();
        c.prepare("SELECT chat, id FROM audios WHERE estado='falhou' AND erro='ferramentas ausentes'")
            .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .unwrap_or_default()
    };
    for (chat, id) in falhas {
        let nome: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let arq = app.pasta.join("audios").join(format!("{nome}.ogg"));
        if arq.exists() {
            let _ = app.banco.registrar_audio(&chat, &id, Some(&arq.to_string_lossy()), "pendente", None);
            let _ = app.fila_audio.try_send(TarefaAudio { chat, id, arquivo: arq });
        }
    }
}

async fn processar(app: &AppRef, f: Option<std::sync::Arc<Ferramentas>>, t: TarefaAudio) {
    let Some(f) = f else {
        let _ = app.banco.definir_texto(&t.chat, &t.id, "[áudio — transcrição não configurada]");
        let _ = app.banco.registrar_audio(&t.chat, &t.id, None, "falhou", Some("ferramentas ausentes"));
        return;
    };
    if !t.arquivo.exists() {
        let _ = app.banco.registrar_audio(&t.chat, &t.id, None, "falhou", Some("arquivo sumiu"));
        let _ = app.banco.definir_texto(&t.chat, &t.id, "[áudio não pôde ser transcrito]");
        return;
    }
    let threads = app.cfg.transcricao.threads;
    let arq = t.arquivo.clone();
    let r = tokio::task::spawn_blocking(move || transcrever(&f, &arq, threads)).await;
    match r {
        Ok(Ok(texto)) => {
            let _ = app.banco.definir_texto(&t.chat, &t.id, &texto);
            let _ = app.banco.registrar_audio(&t.chat, &t.id, None, "transcrito", None);
            info!("Áudio transcrito ({} caracteres)", texto.chars().count());
        }
        Ok(Err(e)) => {
            warn!("Falha na transcrição: {e}");
            let _ = app.banco.registrar_audio(&t.chat, &t.id, None, "falhou", Some(&e.to_string()));
            let _ = app.banco.definir_texto(&t.chat, &t.id, "[áudio não pôde ser transcrito]");
        }
        Err(e) => warn!("Tarefa de transcrição interrompida: {e}"),
    }
    if app.cfg.transcricao.apagar_audio_depois {
        let _ = std::fs::remove_file(&t.arquivo);
    }
}
