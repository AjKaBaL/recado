//! Descobre como é este computador (memória, placa de vídeo, processador) e
//! escolhe as opções da IA e da transcrição que funcionam bem nele.
//!
//! Vale enquanto `ajuste_automatico = true` no config.toml (o padrão). Com `false`,
//! o assistente usa exatamente o que estiver escrito no arquivo.

use crate::config::Config;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Computador {
    /// Memória total, em GB.
    pub ram_gb: f64,
    /// Nome da placa de vídeo NVIDIA (vazio = nenhuma encontrada).
    pub placa: String,
    /// Memória da placa de vídeo, em GB (0 = sem placa NVIDIA).
    pub vram_gb: f64,
    pub nucleos: usize,
}

/// O que o assistente escolheu para este computador.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Ajuste {
    pub modelo_ia: String,
    pub pensar: bool,
    pub contexto: u32,
    /// "small" ou "base" (modelo de transcrição preferido).
    pub modelo_audio: String,
    pub threads_audio: u32,
}

pub fn detectar() -> Computador {
    let (placa, vram_gb) = placa_nvidia().unwrap_or_default();
    Computador {
        ram_gb: memoria_gb(),
        placa,
        vram_gb,
        nucleos: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
    }
}

/// Regras simples, pensando em quem não tem placa de vídeo:
/// - 16 GB (o Windows mostra ~15,x) ou placa de 8 GB → modelo completo (qwen3, 8b);
/// - 12 GB ou placa de 4 GB → qwen3:4b;
/// - menos → qwen3:1.7b (mais simples, mas roda).
pub fn recomendar(c: &Computador) -> Ajuste {
    let ram = c.ram_gb;
    let modelo_ia = if ram >= 14.0 || c.vram_gb >= 8.0 {
        "qwen3"
    } else if ram >= 10.0 || (c.vram_gb >= 4.0 && ram >= 7.0) {
        "qwen3:4b"
    } else {
        "qwen3:1.7b"
    };
    Ajuste {
        modelo_ia: modelo_ia.into(),
        // "Pensar" deixa a análise mais precisa, mas é lento em PC fraco.
        pensar: ram >= 14.0,
        contexto: if ram >= 10.0 { 8192 } else { 4096 },
        modelo_audio: if ram >= 10.0 { "small" } else { "base" }.into(),
        threads_audio: ((c.nucleos / 2) as u32).clamp(2, 8),
    }
}

/// Aplica o ajuste na configuração (só na memória; o arquivo não muda).
pub fn aplicar(cfg: &mut Config, a: &Ajuste) {
    cfg.ia.modelo = a.modelo_ia.clone();
    cfg.ia.pensar = a.pensar;
    cfg.ia.contexto = a.contexto;
    cfg.transcricao.threads = a.threads_audio;
    cfg.transcricao.preferir_modelo = a.modelo_audio.clone();
}

/// Texto curto para o painel, ex.: "16 GB de memória · NVIDIA RTX 3050 (4 GB) · 12 núcleos".
pub fn descrever(c: &Computador) -> String {
    let placa = if c.placa.is_empty() {
        "sem placa de vídeo NVIDIA".to_string()
    } else {
        format!("{} ({:.0} GB)", c.placa, c.vram_gb)
    };
    format!("{:.0} GB de memória · {} · {} núcleos", c.ram_gb, placa, c.nucleos)
}

#[cfg(windows)]
fn memoria_gb() -> f64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) != 0 {
            return m.ullTotalPhys as f64 / 1024f64.powi(3);
        }
    }
    8.0
}

#[cfg(not(windows))]
fn memoria_gb() -> f64 {
    8.0
}

/// Placa NVIDIA (o Ollama usa a placa de vídeo principalmente nelas).
fn placa_nvidia() -> Option<(String, f64)> {
    let mut cmd = std::process::Command::new("nvidia-smi");
    cmd.args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // sem janela
    }
    let saida = cmd.output().ok().filter(|s| s.status.success())?;
    let txt = String::from_utf8_lossy(&saida.stdout);
    let linha = txt.lines().next()?;
    let (nome, mb) = linha.rsplit_once(',')?;
    let mb: f64 = mb.trim().parse().ok()?;
    let nome = nome.trim().trim_start_matches("NVIDIA GeForce ").trim_start_matches("NVIDIA ");
    Some((format!("NVIDIA {nome}"), mb / 1024.0))
}

#[cfg(test)]
mod testes {
    use super::*;

    fn pc(ram: f64, vram: f64) -> Computador {
        Computador { ram_gb: ram, vram_gb: vram, placa: String::new(), nucleos: 8 }
    }

    #[test]
    fn escolhe_o_modelo_pelo_computador() {
        assert_eq!(recomendar(&pc(15.7, 4.0)).modelo_ia, "qwen3"); // este notebook
        assert_eq!(recomendar(&pc(31.8, 0.0)).modelo_ia, "qwen3");
        assert_eq!(recomendar(&pc(11.7, 0.0)).modelo_ia, "qwen3:4b");
        assert_eq!(recomendar(&pc(7.8, 0.0)).modelo_ia, "qwen3:1.7b");
        assert_eq!(recomendar(&pc(7.8, 4.0)).modelo_ia, "qwen3:4b");
        let fraco = recomendar(&pc(7.8, 0.0));
        assert!(!fraco.pensar);
        assert_eq!((fraco.contexto, fraco.modelo_audio.as_str()), (4096, "base"));
        assert_eq!(recomendar(&pc(15.7, 4.0)).threads_audio, 4);
    }
}
