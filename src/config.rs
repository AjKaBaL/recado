//! Configuração do aplicativo (arquivo `config.toml` na pasta de dados).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Prefixo dos comandos digitados na conversa consigo mesmo.
    pub prefixo_comando: String,
    /// Porta do painel local (só acessível neste computador).
    pub porta_painel: u16,
    /// Abrir o painel no navegador ao iniciar.
    pub abrir_navegador: bool,
    /// Nome de um grupo SEU (só você nele) para ser a conversa do assistente.
    /// Vazio = usar a conversa consigo mesmo. A escolha feita no painel tem prioridade.
    pub grupo_assistente: String,
    /// Descobrir como é o computador (memória, placa de vídeo, processador) e escolher
    /// sozinho o modelo da IA, o "pensar", o contexto e a transcrição. Com `false`, valem
    /// os valores escritos abaixo.
    pub ajuste_automatico: bool,
    /// Transcrição de áudio (whisper.cpp local).
    pub transcricao: ConfigTranscricao,
    /// Inteligência artificial local (Ollama).
    pub ia: ConfigIa,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigTranscricao {
    pub ativada: bool,
    /// Caminho do executável `whisper-cli.exe`. Vazio = procurar na pasta `ferramentas`.
    pub whisper_exe: String,
    /// Caminho do modelo `ggml-*.bin`. Vazio = procurar na pasta `ferramentas`.
    pub modelo: String,
    /// Apagar o arquivo de áudio depois de transcrever.
    pub apagar_audio_depois: bool,
    /// Quantos núcleos do processador o whisper pode usar.
    pub threads: u32,
    /// Modelo preferido quando houver mais de um na pasta ("small", "base"…). Vazio = o maior.
    pub preferir_modelo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigIa {
    pub ativada: bool,
    /// Endereço do Ollama (local).
    pub url_ollama: String,
    /// Modelo do Ollama usado para análise (ex.: "qwen3", "qwen2.5:3b" em PCs mais fracos).
    pub modelo: String,
    /// Deixar o modelo "pensar" antes de responder (mais preciso, mais lento).
    /// Só tem efeito em modelos com esse recurso (ex.: qwen3).
    pub pensar: bool,
    /// "Pensar" também ao responder perguntas (`!pergunta`). Desligado responde bem mais rápido.
    pub pensar_nas_perguntas: bool,
    /// Tamanho da "memória de trabalho" da IA (tokens). Menor = mais leve.
    pub contexto: u32,
    /// Fechar o Ollama quando o assistente fechar.
    pub fechar_ollama_junto: bool,
    /// Segundos de silêncio numa conversa antes de analisá-la.
    pub espera_analise_seg: u64,
    /// Enviar alertas de importância "alta" e "crítica" na hora.
    pub alertas_imediatos: bool,
    /// Horário do resumo diário (HH:MM). Vazio = desligado.
    pub horario_resumo_diario: String,
    /// Horário de silêncio (alertas não críticos esperam). Ex.: "22:30-07:00". Vazio = sem silêncio.
    pub silencio: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            prefixo_comando: "!".into(),
            porta_painel: 7878,
            abrir_navegador: true,
            grupo_assistente: String::new(),
            ajuste_automatico: true,
            transcricao: ConfigTranscricao::default(),
            ia: ConfigIa::default(),
        }
    }
}

impl Default for ConfigTranscricao {
    fn default() -> Self {
        Self {
            ativada: true,
            whisper_exe: String::new(),
            modelo: String::new(),
            apagar_audio_depois: true,
            threads: 4,
            preferir_modelo: String::new(),
        }
    }
}

impl Default for ConfigIa {
    fn default() -> Self {
        Self {
            ativada: true,
            url_ollama: "http://127.0.0.1:11434".into(),
            modelo: "qwen3".into(),
            pensar: true,
            pensar_nas_perguntas: false,
            contexto: 8192,
            fechar_ollama_junto: true,
            espera_analise_seg: 120,
            alertas_imediatos: true,
            horario_resumo_diario: "21:00".into(),
            silencio: "23:00-07:00".into(),
        }
    }
}

/// Pasta onde ficam sessão, banco e configurações:
/// `%LOCALAPPDATA%\Recado` (ou a variável `ASSISTENTE_DADOS`). Quem já usava a versão
/// com o nome antigo continua com a pasta `AssistenteWhatsApp`.
pub fn pasta_dados() -> PathBuf {
    if let Ok(p) = std::env::var("ASSISTENTE_DADOS") {
        return PathBuf::from(p);
    }
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let antiga = base.join("AssistenteWhatsApp");
    if antiga.join("config.toml").exists() && !base.join("Recado").exists() {
        return antiga;
    }
    base.join("Recado")
}

impl Config {
    pub fn carregar(pasta: &Path) -> Result<Self> {
        let arq = pasta.join("config.toml");
        if !arq.exists() {
            let cfg = Config::default();
            cfg.salvar(pasta)?;
            return Ok(cfg);
        }
        let txt = std::fs::read_to_string(&arq).context("lendo config.toml")?;
        let cfg: Config = toml::from_str(&txt).context("config.toml inválido")?;
        // Regrava para incluir opções novas com o valor padrão.
        cfg.salvar(pasta)?;
        Ok(cfg)
    }

    pub fn salvar(&self, pasta: &Path) -> Result<()> {
        let txt = format!(
            "# Configurações do Recado (assistente pessoal para WhatsApp)\n\
             # Feche o assistente antes de editar este arquivo.\n\n{}",
            toml::to_string_pretty(self)?
        );
        std::fs::write(pasta.join("config.toml"), txt)?;
        Ok(())
    }
}
