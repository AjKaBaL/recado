//! Estado compartilhado do aplicativo.

use crate::banco::Banco;
use crate::config::Config;
use crate::whatsapp::ConexaoWa;
use chrono::{Local, TimeZone};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Default)]
pub struct EstadoConexao {
    /// Texto do QR Code atual (nunca vai para log).
    #[serde(skip)]
    pub qr: Option<String>,
    pub conectado: bool,
    pub logado: bool,
    pub nome_conta: String,
    /// Número mascarado da conta (ex.: 55119****1234).
    pub conta_mascarada: String,
    pub ultima_msg: Option<i64>,
    pub ultimo_evento: String,
    pub historico_progresso: Option<u32>,
    pub desconectado_pelo_usuario: bool,
}

/// Áudio baixado aguardando transcrição.
#[derive(Debug, Clone)]
pub struct TarefaAudio {
    pub chat: String,
    pub id: String,
    pub arquivo: PathBuf,
}

/// Situação do segundo número (o que manda os avisos para o celular tocar).
#[derive(Debug, Clone, Serialize, Default)]
pub struct EstadoSegundo {
    #[serde(skip)]
    pub qr: Option<String>,
    pub conectado: bool,
    pub conta_mascarada: String,
    pub ultimo_evento: String,
}

pub struct App {
    pub pasta: PathBuf,
    pub cfg: Config,
    pub banco: Banco,
    pub estado: RwLock<EstadoConexao>,
    pub conexao: RwLock<Option<ConexaoWa>>,
    pub fila_audio: mpsc::Sender<TarefaAudio>,
    pub ia_disponivel: AtomicBool,
    pub transcricao_disponivel: AtomicBool,
    /// Avisado quando o usuário pede para fechar o assistente.
    pub sair: tokio::sync::Notify,
    /// Pedido de fechar (a bandeja tira o ícone e avisa `sair`).
    pub pedido_sair: AtomicBool,
    pub bandeja_ativa: AtomicBool,
    /// Quando chegou o último pacote de histórico do celular (milissegundos).
    pub historico_recebido: std::sync::atomic::AtomicI64,
    /// Segundo número (opcional): só envia avisos para o número principal do usuário.
    pub segundo: RwLock<Option<ConexaoWa>>,
    pub estado_segundo: RwLock<EstadoSegundo>,
    /// Avisado quando o usuário liga ou desliga o segundo número.
    pub mudou_segundo: tokio::sync::Notify,
    /// Como é este computador e o que foi escolhido para ele (None = ajuste manual).
    pub computador: crate::computador::Computador,
    pub ajuste: Option<crate::computador::Ajuste>,
}

impl App {
    pub fn conexao(&self) -> Option<ConexaoWa> {
        self.conexao.read().ok().and_then(|g| g.clone())
    }

    pub fn definir_conexao(&self, c: ConexaoWa) {
        if let Ok(mut g) = self.conexao.write() {
            *g = Some(c);
        }
    }

    pub fn alterar_estado(&self, f: impl FnOnce(&mut EstadoConexao)) {
        if let Ok(mut g) = self.estado.write() {
            f(&mut g);
        }
    }

    pub fn ler_estado(&self) -> EstadoConexao {
        self.estado.read().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn segundo_ligado(&self) -> bool {
        self.banco.estado("segundo_numero").as_deref() == Some("1")
    }

    pub fn segundo(&self) -> Option<ConexaoWa> {
        self.segundo.read().ok().and_then(|g| g.clone())
    }

    pub fn alterar_segundo(&self, f: impl FnOnce(&mut EstadoSegundo)) {
        if let Ok(mut g) = self.estado_segundo.write() {
            f(&mut g);
        }
    }

    pub fn ler_segundo(&self) -> EstadoSegundo {
        self.estado_segundo.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Diz se `chat` (na conta principal) é a conversa com o segundo número.
    pub fn eh_chat_do_segundo(&self, chat: &str) -> bool {
        ["segundo_pn", "segundo_lid"]
            .iter()
            .any(|k| self.banco.estado(k).is_some_and(|j| j == chat))
    }

    pub fn pausado(&self) -> bool {
        self.banco.estado("pausado").as_deref() == Some("1")
    }

    pub fn definir_pausado(&self, p: bool) {
        let _ = self.banco.definir_estado("pausado", if p { "1" } else { "0" });
    }

    pub fn envio_habilitado(&self) -> bool {
        self.banco.estado("envio_habilitado").as_deref() == Some("1")
            && self.banco.estado("chat_proprio").is_some()
    }

    pub fn ia_ok(&self) -> bool {
        self.ia_disponivel.load(Ordering::Relaxed)
    }

    /// Fecha o assistente (pela bandeja, se ela existir, para tirar o ícone).
    pub fn fechar(&self) {
        self.pedido_sair.store(true, Ordering::Relaxed);
        if !self.bandeja_ativa.load(Ordering::Relaxed) {
            self.sair.notify_one();
        }
    }

    /// Grupo escolhido como conversa do assistente (None = conversa consigo mesmo).
    pub fn grupo_escolhido(&self) -> Option<String> {
        self.banco
            .estado("conversa_assistente")
            .filter(|j| j.ends_with("@g.us"))
    }

    /// Diz se `chat` é a conversa onde o assistente conversa com o usuário.
    pub fn eh_conversa_do_assistente(&self, con: &ConexaoWa, chat: &str) -> bool {
        match self.grupo_escolhido() {
            Some(g) => g == chat,
            None => con.eh_conversa_propria(chat),
        }
    }

    /// Troca a conversa do assistente ("propria" ou JID de grupo). Exige novo `!ativar`.
    pub fn escolher_conversa(&self, valor: &str) {
        if self.banco.estado("conversa_assistente").as_deref() == Some(valor) {
            return;
        }
        let _ = self.banco.definir_estado("conversa_assistente", valor);
        let _ = self.banco.remover_estado("envio_habilitado");
        let _ = self.banco.remover_estado("chat_proprio");
        let _ = self.banco.remover_estado("erro_ativacao");
    }

    /// Se o usuário configurou o nome do grupo e ainda não escolheu no painel,
    /// adota o grupo com esse nome assim que ele aparecer.
    pub fn talvez_adotar_grupo(&self, chat: &str, nome: &str) {
        if self.cfg.grupo_assistente.trim().is_empty()
            || !chat.ends_with("@g.us")
            || self.banco.estado("conversa_assistente").is_some()
        {
            return;
        }
        if nome_normalizado(nome) == nome_normalizado(&self.cfg.grupo_assistente) {
            self.escolher_conversa(chat);
            tracing::info!("Grupo do assistente encontrado pelo nome configurado");
        }
    }
}

/// "Meu- WhatsApp  Assistente" vira "meuwhatsappassistente" (para comparar nomes).
pub fn nome_normalizado(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

pub type AppRef = Arc<App>;

pub fn agora() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn agora_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// "07/11 14:32"
pub fn data_curta(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%d/%m %H:%M").to_string())
        .unwrap_or_default()
}

/// Início do dia de hoje (horário local), em segundos UTC.
pub fn inicio_de_hoje() -> i64 {
    let hoje = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
    Local
        .from_local_datetime(&hoje)
        .single()
        .map(|d| d.timestamp())
        .unwrap_or_else(agora)
}

/// Mostra só o começo e o fim do número (para o painel, sem expor o número inteiro em logs).
pub fn mascarar(numero: &str) -> String {
    let n: Vec<char> = numero.chars().collect();
    if n.len() <= 6 {
        return "****".into();
    }
    let ini: String = n[..4].iter().collect();
    let fim: String = n[n.len() - 4..].iter().collect();
    format!("{ini}****{fim}")
}
