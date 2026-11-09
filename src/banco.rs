//! Banco de dados local do assistente (SQLite).
//!
//! Este banco é do assistente. A sessão do WhatsApp fica em outro arquivo,
//! gerenciado pela biblioteca de conexão.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

pub struct Banco {
    con: Mutex<Connection>,
}

/// Mensagem já normalizada, independente da biblioteca de WhatsApp.
#[derive(Debug, Clone, Serialize)]
pub struct Mensagem {
    pub chat: String,
    pub id: String,
    pub remetente: String,
    pub nome_remetente: String,
    pub de_mim: bool,
    /// Segundos desde 1970 (UTC).
    pub data: i64,
    /// texto, audio, imagem, video, documento, figurinha, contato, localizacao, enquete, outro
    pub tipo: String,
    pub texto: Option<String>,
    /// "ao_vivo" ou "historico"
    pub origem: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Conversa {
    pub jid: String,
    pub nome: String,
    pub grupo: bool,
    pub monitorada: bool,
    /// 0 = normal, 1 = prioritária
    pub prioridade: i64,
    pub ultima_msg: Option<i64>,
    pub total_msgs: i64,
    /// Verdadeiro se o nome veio da agenda do celular (senão, é o nome que a pessoa pôs no WhatsApp).
    pub nome_da_agenda: bool,
    /// Número de telefone (quando conhecido), só dígitos.
    pub numero: Option<String>,
    /// Data da mensagem mais antiga guardada (até onde vai o histórico).
    pub primeira_msg: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MensagemExibicao {
    pub chat: String,
    pub nome_chat: String,
    pub id: String,
    pub nome_remetente: String,
    pub de_mim: bool,
    pub data: i64,
    pub tipo: String,
    pub texto: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Contadores {
    pub mensagens: i64,
    pub conversas: i64,
    pub monitoradas: i64,
    pub historico_msgs: i64,
    pub historico_de: Option<i64>,
    pub historico_ate: Option<i64>,
    pub audios_transcritos: i64,
    pub audios_pendentes: i64,
}

const ESQUEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS conversas (
    jid         TEXT PRIMARY KEY,
    nome        TEXT NOT NULL DEFAULT '',
    grupo       INTEGER NOT NULL DEFAULT 0,
    monitorada  INTEGER NOT NULL DEFAULT 1,
    prioridade  INTEGER NOT NULL DEFAULT 0,
    ultima_msg  INTEGER
);

-- Nomes salvos na agenda do celular (vêm da sincronização de contatos do WhatsApp).
CREATE TABLE IF NOT EXISTS agenda (
    jid   TEXT PRIMARY KEY,
    nome  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS mensagens (
    rowid_      INTEGER PRIMARY KEY AUTOINCREMENT,
    chat        TEXT NOT NULL,
    id          TEXT NOT NULL,
    remetente   TEXT NOT NULL,
    nome_remetente TEXT NOT NULL DEFAULT '',
    de_mim      INTEGER NOT NULL,
    data        INTEGER NOT NULL,
    tipo        TEXT NOT NULL,
    texto       TEXT,
    origem      TEXT NOT NULL,
    analisada   INTEGER NOT NULL DEFAULT 0,
    UNIQUE(chat, id)
);
CREATE INDEX IF NOT EXISTS idx_msg_chat_data ON mensagens(chat, data);
CREATE INDEX IF NOT EXISTS idx_msg_data ON mensagens(data);

CREATE VIRTUAL TABLE IF NOT EXISTS mensagens_fts USING fts5(
    texto, content='mensagens', content_rowid='rowid_',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS msg_ai AFTER INSERT ON mensagens WHEN new.texto IS NOT NULL BEGIN
    INSERT INTO mensagens_fts(rowid, texto) VALUES (new.rowid_, new.texto);
END;
CREATE TRIGGER IF NOT EXISTS msg_ad AFTER DELETE ON mensagens WHEN old.texto IS NOT NULL BEGIN
    INSERT INTO mensagens_fts(mensagens_fts, rowid, texto) VALUES ('delete', old.rowid_, old.texto);
END;
CREATE TRIGGER IF NOT EXISTS msg_au AFTER UPDATE OF texto ON mensagens BEGIN
    INSERT INTO mensagens_fts(mensagens_fts, rowid, texto)
        SELECT 'delete', old.rowid_, old.texto WHERE old.texto IS NOT NULL;
    INSERT INTO mensagens_fts(rowid, texto)
        SELECT new.rowid_, new.texto WHERE new.texto IS NOT NULL;
END;

-- Áudios aguardando/concluídos de transcrição.
CREATE TABLE IF NOT EXISTS audios (
    chat        TEXT NOT NULL,
    id          TEXT NOT NULL,
    arquivo     TEXT,
    estado      TEXT NOT NULL,   -- pendente, transcrito, falhou
    erro        TEXT,
    PRIMARY KEY(chat, id)
);

-- IDs das mensagens que o próprio assistente enviou (para nunca reagir a elas).
CREATE TABLE IF NOT EXISTS enviadas_pelo_assistente (
    id          TEXT PRIMARY KEY,
    data        INTEGER NOT NULL
);

-- Estado simples chave/valor.
CREATE TABLE IF NOT EXISTS estado (
    chave       TEXT PRIMARY KEY,
    valor       TEXT NOT NULL
);

-- Assuntos (fios) que ligam mensagens, inclusive de dias e conversas diferentes.
CREATE TABLE IF NOT EXISTS assuntos (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    titulo      TEXT NOT NULL,
    resumo      TEXT NOT NULL DEFAULT '',
    estado      TEXT NOT NULL DEFAULT 'aberto',  -- aberto, resolvido
    criado      INTEGER NOT NULL,
    atualizado  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS assunto_mensagens (
    assunto     INTEGER NOT NULL REFERENCES assuntos(id) ON DELETE CASCADE,
    chat        TEXT NOT NULL,
    id          TEXT NOT NULL,
    PRIMARY KEY(assunto, chat, id)
);

-- Pendências / lembretes / compromissos extraídos pela IA.
CREATE TABLE IF NOT EXISTS itens (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    tipo        TEXT NOT NULL,       -- tarefa, compromisso, pagamento, promessa, data, pedido, pergunta
    descricao   TEXT NOT NULL,
    prazo_texto TEXT,                -- como foi dito ("sexta-feira")
    prazo       INTEGER,             -- data absoluta (se não for ambígua)
    importancia TEXT NOT NULL,       -- critica, alta, media, baixa
    estado      TEXT NOT NULL DEFAULT 'pendente', -- pendente, concluido, cancelado
    chat        TEXT NOT NULL,
    msg_id      TEXT,
    assunto     INTEGER REFERENCES assuntos(id) ON DELETE SET NULL,
    criado      INTEGER NOT NULL,
    alertado    INTEGER NOT NULL DEFAULT 0,
    lembrado    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_itens_estado ON itens(estado, prazo);
"#;

impl Banco {
    pub fn abrir(arquivo: &Path) -> Result<Self> {
        let con = Connection::open(arquivo)?;
        con.execute_batch(ESQUEMA)?;
        // Colunas novas em bancos antigos.
        for (col, tipo) in [("nome_agenda", "TEXT"), ("nome_whatsapp", "TEXT"), ("numero", "TEXT")] {
            let existe: bool = con
                .prepare("SELECT 1 FROM pragma_table_info('conversas') WHERE name=?1")?
                .exists([col])?;
            if !existe {
                con.execute_batch(&format!("ALTER TABLE conversas ADD COLUMN {col} {tipo}"))?;
            }
        }
        // Espaço em disco: o arquivo temporário (-wal) volta a no máximo 4 MB depois de
        // cada gravação no banco principal, e é esvaziado ao abrir.
        con.execute_batch(
            "PRAGMA journal_size_limit = 4194304;
             PRAGMA wal_checkpoint(TRUNCATE);",
        )?;
        Ok(Self { con: Mutex::new(con) })
    }

    fn c(&self) -> MutexGuard<'_, Connection> {
        self.con.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---------- estado chave/valor ----------

    pub fn estado(&self, chave: &str) -> Option<String> {
        self.c()
            .query_row("SELECT valor FROM estado WHERE chave=?1", [chave], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn definir_estado(&self, chave: &str, valor: &str) -> Result<()> {
        self.c().execute(
            "INSERT INTO estado(chave,valor) VALUES(?1,?2)
             ON CONFLICT(chave) DO UPDATE SET valor=excluded.valor",
            params![chave, valor],
        )?;
        Ok(())
    }

    pub fn remover_estado(&self, chave: &str) -> Result<()> {
        self.c().execute("DELETE FROM estado WHERE chave=?1", [chave])?;
        Ok(())
    }

    // ---------- conversas ----------

    /// Garante que a conversa existe. Atualiza o nome se vier um nome melhor.
    pub fn registrar_conversa(&self, jid: &str, nome: Option<&str>, grupo: bool) -> Result<()> {
        let nome = nome.unwrap_or("").trim();
        self.c().execute(
            "INSERT INTO conversas(jid,nome,grupo) VALUES(?1,?2,?3)
             ON CONFLICT(jid) DO UPDATE SET
                nome = CASE WHEN excluded.nome <> '' THEN excluded.nome ELSE conversas.nome END",
            params![jid, nome, grupo],
        )?;
        Ok(())
    }

    // ---------- nomes dos contatos ----------
    // Nome mostrado = nome da agenda; se não houver, o nome que a pessoa pôs no WhatsApp.

    /// Nome que a pessoa colocou no WhatsApp ("push name"). Só atualiza conversas que já existem.
    /// `numero` (só dígitos, se conhecido) acha a mesma pessoa guardada com outro identificador.
    pub fn definir_nome_whatsapp(&self, jid: &str, nome: &str, numero: Option<&str>) -> Result<()> {
        let nome = nome.trim();
        if nome.is_empty() {
            return Ok(());
        }
        self.c().execute(
            "UPDATE conversas SET nome_whatsapp=?2,
                    nome = CASE WHEN COALESCE(nome_agenda,'') <> '' THEN nome_agenda ELSE ?2 END
             WHERE (jid=?1 OR (?3 IS NOT NULL AND numero=?3))
               AND grupo=0 AND jid NOT LIKE '%@g.us' AND nome <> 'Eu (mensagens para mim)'",
            params![jid, nome, numero],
        )?;
        Ok(())
    }

    /// Nome salvo na agenda. Guarda para conversas futuras e atualiza as que já existem.
    pub fn definir_nome_agenda(&self, jids: &[String], nome: &str) -> Result<()> {
        let nome = nome.trim();
        if nome.is_empty() {
            return Ok(());
        }
        let c = self.c();
        for jid in jids {
            c.execute(
                "INSERT INTO agenda(jid,nome) VALUES(?1,?2) ON CONFLICT(jid) DO UPDATE SET nome=excluded.nome",
                params![jid, nome],
            )?;
            c.execute(
                "UPDATE conversas SET nome_agenda=?2, nome=?2
                 WHERE jid=?1 AND grupo=0 AND nome <> 'Eu (mensagens para mim)'",
                params![jid, nome],
            )?;
        }
        Ok(())
    }

    pub fn definir_numero(&self, jid: &str, numero: &str) -> Result<()> {
        let c = self.c();
        c.execute("UPDATE conversas SET numero=?2 WHERE jid=?1", params![jid, numero])?;
        // Se o número tem nome na agenda, usa também para esta conversa.
        c.execute(
            "UPDATE conversas SET nome_agenda=(SELECT nome FROM agenda WHERE jid=?2), nome=(SELECT nome FROM agenda WHERE jid=?2)
             WHERE jid=?1 AND grupo=0 AND COALESCE(nome_agenda,'')='' AND nome <> 'Eu (mensagens para mim)'
               AND EXISTS (SELECT 1 FROM agenda WHERE jid=?2)",
            params![jid, format!("{numero}@s.whatsapp.net")],
        )?;
        Ok(())
    }

    /// Contatos com o número ainda desconhecido (para procurar pelo LID).
    pub fn contatos_sem_numero(&self) -> Result<Vec<String>> {
        let c = self.c();
        let mut st = c.prepare("SELECT jid FROM conversas WHERE grupo=0 AND jid LIKE '%@lid' AND numero IS NULL")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
        Ok(v)
    }

    /// Preenche nomes a partir do que já está guardado: agenda e nomes vistos nas mensagens.
    pub fn completar_nomes(&self) -> Result<()> {
        self.c().execute_batch(
            "UPDATE conversas SET numero = substr(jid, 1, instr(jid,'@')-1)
               WHERE numero IS NULL AND jid LIKE '%@s.whatsapp.net';
             UPDATE conversas SET nome_whatsapp = (
                 SELECT m.nome_remetente FROM mensagens m
                 WHERE m.remetente = conversas.jid AND m.de_mim = 0 AND m.nome_remetente <> ''
                 ORDER BY m.data DESC LIMIT 1)
               WHERE grupo=0 AND COALESCE(nome_whatsapp,'')='';
             UPDATE conversas SET nome_agenda = (SELECT nome FROM agenda a WHERE a.jid = conversas.jid)
               WHERE grupo=0 AND COALESCE(nome_agenda,'')='';
             UPDATE conversas SET nome_agenda = (SELECT nome FROM agenda a WHERE a.jid = conversas.numero || '@s.whatsapp.net')
               WHERE grupo=0 AND COALESCE(nome_agenda,'')='' AND numero IS NOT NULL;
             UPDATE conversas SET nome = CASE
                   WHEN COALESCE(nome_agenda,'') <> '' THEN nome_agenda
                   WHEN COALESCE(nome_whatsapp,'') <> '' THEN nome_whatsapp
                   ELSE nome END
               WHERE grupo=0 AND nome <> 'Eu (mensagens para mim)';",
        )?;
        Ok(())
    }

    pub fn conversa_monitorada(&self, jid: &str) -> bool {
        self.c()
            .query_row("SELECT monitorada FROM conversas WHERE jid=?1", [jid], |r| {
                r.get::<_, bool>(0)
            })
            .unwrap_or(true)
    }

    pub fn nome_conversa(&self, jid: &str) -> String {
        let nome: Option<String> = self
            .c()
            .query_row("SELECT nome FROM conversas WHERE jid=?1", [jid], |r| r.get(0))
            .optional()
            .ok()
            .flatten();
        match nome {
            Some(n) if !n.is_empty() => n,
            _ => jid.split('@').next().unwrap_or(jid).to_string(),
        }
    }

    pub fn definir_monitoramento(&self, jid: &str, monitorada: bool, prioridade: i64) -> Result<()> {
        self.c().execute(
            "UPDATE conversas SET monitorada=?2, prioridade=?3 WHERE jid=?1",
            params![jid, monitorada, prioridade],
        )?;
        Ok(())
    }

    /// Mensagem mais antiga guardada de uma conversa: (id, de_mim, data).
    pub fn mensagem_mais_antiga(&self, chat: &str) -> Option<(String, bool, i64)> {
        self.c()
            .query_row(
                "SELECT id, de_mim, data FROM mensagens WHERE chat=?1 ORDER BY data ASC, rowid_ ASC LIMIT 1",
                [chat],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn listar_conversas(&self, filtro: &str) -> Result<Vec<Conversa>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT c.jid, c.nome, c.grupo, c.monitorada, c.prioridade, c.ultima_msg,
                    (SELECT COUNT(*) FROM mensagens m WHERE m.chat=c.jid),
                    (SELECT MIN(data) FROM mensagens m WHERE m.chat=c.jid),
                    COALESCE(c.nome_agenda,'') <> '', c.numero
             FROM conversas c
             WHERE (?1 = '' OR c.nome LIKE '%' || ?1 || '%' OR c.jid LIKE '%' || ?1 || '%')
             ORDER BY c.prioridade DESC, c.ultima_msg DESC NULLS LAST
             LIMIT 500",
        )?;
        let v = st
            .query_map([filtro], |r| {
                Ok(Conversa {
                    jid: r.get(0)?,
                    nome: r.get(1)?,
                    grupo: r.get(2)?,
                    monitorada: r.get(3)?,
                    prioridade: r.get(4)?,
                    ultima_msg: r.get(5)?,
                    total_msgs: r.get(6)?,
                    primeira_msg: r.get(7)?,
                    nome_da_agenda: r.get(8)?,
                    numero: r.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    // ---------- mensagens ----------

    /// Grava a mensagem. Retorna `true` se era nova (sem duplicar).
    pub fn salvar_mensagem(&self, m: &Mensagem) -> Result<bool> {
        let c = self.c();
        let n = c.execute(
            "INSERT OR IGNORE INTO mensagens
                (chat,id,remetente,nome_remetente,de_mim,data,tipo,texto,origem)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                m.chat, m.id, m.remetente, m.nome_remetente, m.de_mim, m.data, m.tipo, m.texto,
                m.origem
            ],
        )?;
        c.execute(
            "UPDATE conversas SET ultima_msg = MAX(COALESCE(ultima_msg,0), ?2) WHERE jid=?1",
            params![m.chat, m.data],
        )?;
        Ok(n > 0)
    }

    /// Grava várias mensagens de uma vez (histórico). Retorna quantas eram novas.
    pub fn salvar_lote(&self, lote: &[Mensagem]) -> Result<usize> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let mut novas = 0;
        {
            let mut ins = tx.prepare(
                "INSERT OR IGNORE INTO mensagens
                    (chat,id,remetente,nome_remetente,de_mim,data,tipo,texto,origem,analisada)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1)",
            )?;
            let mut upd = tx.prepare(
                "UPDATE conversas SET ultima_msg = MAX(COALESCE(ultima_msg,0), ?2) WHERE jid=?1",
            )?;
            for m in lote {
                novas += ins.execute(params![
                    m.chat, m.id, m.remetente, m.nome_remetente, m.de_mim, m.data, m.tipo,
                    m.texto, m.origem
                ])?;
                upd.execute(params![m.chat, m.data])?;
            }
        }
        tx.commit()?;
        Ok(novas)
    }

    pub fn definir_texto(&self, chat: &str, id: &str, texto: &str) -> Result<()> {
        self.c().execute(
            "UPDATE mensagens SET texto=?3, analisada=0 WHERE chat=?1 AND id=?2",
            params![chat, id, texto],
        )?;
        Ok(())
    }

    pub fn ultimas_mensagens(&self, chat: Option<&str>, limite: i64) -> Result<Vec<MensagemExibicao>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.chat, COALESCE(NULLIF(c.nome,''), m.chat), m.id, m.nome_remetente, m.de_mim,
                    m.data, m.tipo, m.texto
             FROM mensagens m LEFT JOIN conversas c ON c.jid=m.chat
             WHERE (?1 IS NULL OR m.chat = ?1)
             ORDER BY m.data DESC LIMIT ?2",
        )?;
        let v = st
            .query_map(params![chat, limite], linha_exibicao)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// Busca por palavras em todas as conversas e datas.
    /// Busca para o usuário (`!buscar`, painel): só palavras que importam; primeiro
    /// mensagens com TODAS as palavras, senão com qualquer uma, mais relevantes primeiro.
    pub fn buscar(&self, termos: &str, limite: i64) -> Result<Vec<MensagemExibicao>> {
        let t = termos_importantes(termos);
        let mut v = self.buscar_ranqueado(&t, None, limite)?;
        v.sort_by_key(|m| std::cmp::Reverse(m.data));
        Ok(v)
    }

    /// Busca por relevância (bm25). `chat` limita a uma conversa.
    pub fn buscar_ranqueado(&self, termos: &[String], chat: Option<&str>, limite: i64) -> Result<Vec<MensagemExibicao>> {
        if termos.is_empty() {
            return Ok(vec![]);
        }
        let partes: Vec<String> = termos.iter().take(10).map(|p| format!("\"{}\"*", p.replace('"', ""))).collect();
        let todas = partes.join(" AND ");
        let qualquer = partes.join(" OR ");
        let mut v = self.buscar_fts(&todas, chat, limite)?;
        if (v.len() as i64) < limite / 3 && partes.len() > 1 {
            for m in self.buscar_fts(&qualquer, chat, limite)? {
                if !v.iter().any(|x| x.chat == m.chat && x.id == m.id) && (v.len() as i64) < limite {
                    v.push(m);
                }
            }
        }
        Ok(v)
    }

    fn buscar_fts(&self, consulta: &str, chat: Option<&str>, limite: i64) -> Result<Vec<MensagemExibicao>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.chat, COALESCE(NULLIF(c.nome,''), m.chat), m.id, m.nome_remetente, m.de_mim,
                    m.data, m.tipo, m.texto
             FROM mensagens_fts f
             JOIN mensagens m ON m.rowid_ = f.rowid
             LEFT JOIN conversas c ON c.jid=m.chat
             WHERE mensagens_fts MATCH ?1 AND (?3 IS NULL OR m.chat = ?3)
             ORDER BY f.rank, m.data DESC LIMIT ?2",
        )?;
        let v = st
            .query_map(params![consulta, limite, chat], linha_exibicao)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// Documentos (arquivos) enviados, mais recentes primeiro. `chat` limita a uma conversa.
    pub fn documentos(&self, chat: Option<&str>, limite: i64) -> Result<Vec<MensagemExibicao>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.chat, COALESCE(NULLIF(c.nome,''), m.chat), m.id, m.nome_remetente, m.de_mim,
                    m.data, m.tipo, m.texto
             FROM mensagens m LEFT JOIN conversas c ON c.jid=m.chat
             WHERE m.tipo='documento' AND (?2 IS NULL OR m.chat=?2)
             ORDER BY m.data DESC LIMIT ?1",
        )?;
        let v = st.query_map(params![limite, chat], linha_exibicao)?.collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// Busca antiga por qualquer palavra, mais recentes primeiro (usada na análise).
    pub fn buscar_qualquer(&self, termos: &str, limite: i64) -> Result<Vec<MensagemExibicao>> {
        let consulta = consulta_fts(termos);
        if consulta.is_empty() {
            return Ok(vec![]);
        }
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.chat, COALESCE(NULLIF(c.nome,''), m.chat), m.id, m.nome_remetente, m.de_mim,
                    m.data, m.tipo, m.texto
             FROM mensagens_fts f
             JOIN mensagens m ON m.rowid_ = f.rowid
             LEFT JOIN conversas c ON c.jid=m.chat
             WHERE mensagens_fts MATCH ?1
             ORDER BY m.data DESC LIMIT ?2",
        )?;
        let v = st
            .query_map(params![consulta, limite], linha_exibicao)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// Mensagens de uma conversa num intervalo (para análise e resumos).
    pub fn mensagens_periodo(&self, chat: &str, de: i64, ate: i64, limite: i64) -> Result<Vec<MensagemExibicao>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT * FROM (
               SELECT m.chat, COALESCE(NULLIF(c.nome,''), m.chat), m.id, m.nome_remetente, m.de_mim,
                      m.data, m.tipo, m.texto
               FROM mensagens m LEFT JOIN conversas c ON c.jid=m.chat
               WHERE m.chat=?1 AND m.data BETWEEN ?2 AND ?3
               ORDER BY m.data DESC LIMIT ?4
             ) ORDER BY 6 ASC",
        )?;
        let v = st
            .query_map(params![chat, de, ate, limite], linha_exibicao)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// Conversas monitoradas com mensagens novas ainda não analisadas e paradas há `espera` segundos.
    pub fn conversas_para_analisar(&self, agora: i64, espera: i64) -> Result<Vec<String>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.chat FROM mensagens m JOIN conversas c ON c.jid=m.chat
             WHERE m.analisada=0 AND c.monitorada=1
             GROUP BY m.chat
             HAVING MAX(m.data) <= ?1
             ORDER BY MAX(c.prioridade) DESC, MIN(m.data) ASC",
        )?;
        let v = st
            .query_map([agora - espera], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(v)
    }

    pub fn nao_analisadas(&self, chat: &str, limite: i64) -> Result<Vec<MensagemExibicao>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.chat, COALESCE(NULLIF(c.nome,''), m.chat), m.id, m.nome_remetente, m.de_mim,
                    m.data, m.tipo, m.texto
             FROM mensagens m LEFT JOIN conversas c ON c.jid=m.chat
             WHERE m.chat=?1 AND m.analisada=0
             ORDER BY m.data ASC LIMIT ?2",
        )?;
        let v = st
            .query_map(params![chat, limite], linha_exibicao)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn marcar_analisadas(&self, chat: &str, ids: &[String]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare("UPDATE mensagens SET analisada=1 WHERE chat=?1 AND id=?2")?;
            for id in ids {
                st.execute(params![chat, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Mensagens de hoje por conversa: (nome, quantidade).
    pub fn resumo_contagem(&self, desde: i64) -> Result<Vec<(String, i64)>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT COALESCE(NULLIF(c.nome,''), m.chat), COUNT(*)
             FROM mensagens m LEFT JOIN conversas c ON c.jid=m.chat
             WHERE m.data >= ?1 AND m.de_mim = 0
             GROUP BY m.chat ORDER BY 2 DESC LIMIT 20",
        )?;
        let v = st
            .query_map([desde], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn contadores(&self) -> Contadores {
        let c = self.c();
        let um = |sql: &str| -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
        let opt = |sql: &str| -> Option<i64> { c.query_row(sql, [], |r| r.get(0)).ok().flatten() };
        Contadores {
            mensagens: um("SELECT COUNT(*) FROM mensagens"),
            conversas: um("SELECT COUNT(*) FROM conversas"),
            monitoradas: um("SELECT COUNT(*) FROM conversas WHERE monitorada=1"),
            historico_msgs: um("SELECT COUNT(*) FROM mensagens WHERE origem='historico'"),
            historico_de: opt("SELECT MIN(data) FROM mensagens WHERE origem='historico'"),
            historico_ate: opt("SELECT MAX(data) FROM mensagens WHERE origem='historico'"),
            audios_transcritos: um("SELECT COUNT(*) FROM audios WHERE estado='transcrito'"),
            audios_pendentes: um("SELECT COUNT(*) FROM audios WHERE estado='pendente'"),
        }
    }

    pub fn apagar_historico(&self) -> Result<()> {
        self.c().execute_batch(
            "DELETE FROM assunto_mensagens; DELETE FROM itens; DELETE FROM assuntos;
             DELETE FROM mensagens; DELETE FROM audios; DELETE FROM enviadas_pelo_assistente;
             DELETE FROM agenda; DELETE FROM estado WHERE chave LIKE 'historico_completo:%';
             INSERT INTO mensagens_fts(mensagens_fts) VALUES('rebuild');
             UPDATE conversas SET ultima_msg=NULL;
             VACUUM;",
        )?;
        Ok(())
    }

    // ---------- áudios ----------

    pub fn registrar_audio(&self, chat: &str, id: &str, arquivo: Option<&str>, estado: &str, erro: Option<&str>) -> Result<()> {
        self.c().execute(
            "INSERT INTO audios(chat,id,arquivo,estado,erro) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(chat,id) DO UPDATE SET arquivo=COALESCE(excluded.arquivo, audios.arquivo),
                 estado=excluded.estado, erro=excluded.erro",
            params![chat, id, arquivo, estado, erro],
        )?;
        Ok(())
    }

    // ---------- mensagens do assistente ----------

    pub fn registrar_enviada(&self, id: &str, data: i64) -> Result<()> {
        self.c().execute(
            "INSERT OR IGNORE INTO enviadas_pelo_assistente(id,data) VALUES(?1,?2)",
            params![id, data],
        )?;
        Ok(())
    }

    pub fn foi_enviada_pelo_assistente(&self, id: &str) -> bool {
        self.c()
            .query_row("SELECT 1 FROM enviadas_pelo_assistente WHERE id=?1", [id], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Acesso direto para os módulos de IA/memória.
    pub fn conexao(&self) -> MutexGuard<'_, Connection> {
        self.c()
    }
}

fn linha_exibicao(r: &rusqlite::Row<'_>) -> rusqlite::Result<MensagemExibicao> {
    Ok(MensagemExibicao {
        chat: r.get(0)?,
        nome_chat: r.get(1)?,
        id: r.get(2)?,
        nome_remetente: r.get(3)?,
        de_mim: r.get(4)?,
        data: r.get(5)?,
        tipo: r.get(6)?,
        texto: r.get(7)?,
    })
}

/// Transforma texto livre numa consulta FTS5 segura (cada palavra entre aspas, com prefixo).
/// Palavras que não ajudam a buscar (perguntas, artigos, verbos genéricos).
const PALAVRAS_VAZIAS: &[&str] = &[
    "a", "o", "as", "os", "um", "uma", "uns", "umas", "de", "da", "do", "das", "dos", "em", "na", "no", "nas", "nos",
    "por", "para", "pra", "pro", "com", "sem", "sob", "sobre", "entre", "ate", "e", "ou", "mas", "que", "se", "nao",
    "sim", "ja", "mais", "menos", "muito", "pouco", "como", "qual", "quais", "quem", "quando", "onde", "porque",
    "por que", "oque", "isso", "isto", "esse", "essa", "este", "esta", "aquele", "aquela", "ele", "ela", "eles",
    "elas", "eu", "me", "mim", "meu", "minha", "meus", "minhas", "voce", "voces", "seu", "sua", "seus", "suas",
    "lhe", "nosso", "nossa", "tem", "ter", "teve", "tinha", "foi", "era", "ser", "esta", "estao", "estava", "sao",
    "consegue", "consegues", "conseguir", "identifica", "identificar", "encontra", "encontrar", "acha", "achar",
    "procura", "procurar", "busca", "buscar", "pesquisa", "pesquisar", "mostra", "mostrar", "fala", "falar",
    "falaram", "falou", "disse", "disseram", "sabe", "saber", "pode", "poderia", "favor", "algum", "alguma",
    "alguem", "algo", "tudo", "todo", "toda", "todos", "todas", "coisa", "coisas", "aqui", "ali", "la",
    "mensagem", "mensagens", "conversa", "conversas", "grupo", "grupos", "chat", "whatsapp", "zap",
    "assunto", "assuntos", "teve", "houve", "existe", "existem", "ha", "hoje", "ontem", "agora",
    "mandou", "mandaram", "mandado", "mandados", "mandada", "mandadas", "mandar", "enviou", "enviaram",
    "enviado", "enviados", "enviada", "enviadas", "enviar", "foram", "quais", "falando", "postou", "postaram",
    "revise", "revisar", "revisa", "verifique", "verificar", "veja", "ver", "olhe", "resuma", "resumir", "liste",
    "listar", "organizar", "desse", "dessa", "deste", "desta", "nesse", "nessa", "neste", "nesta", "meus", "esses",
];

/// Tira acentos e deixa em minúsculas ("Matérias" vira "materias").
pub fn sem_acento(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            outro => outro,
        })
        .collect()
}

/// Palavras de uma pergunta ou busca que valem a pena procurar.
pub fn termos_importantes(texto: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for p in sem_acento(texto).split(|c: char| !c.is_alphanumeric()) {
        if p.chars().count() < 2 || PALAVRAS_VAZIAS.contains(&p) {
            continue;
        }
        // Plural simples: "pdfs" vira "pdf", "materias" vira "materia" (a busca já acha o plural).
        let p = if p.chars().count() > 3 && p.ends_with('s') && !p.ends_with("ss") { &p[..p.len() - 1] } else { p };
        if !v.iter().any(|x| x == p) {
            v.push(p.to_string());
        }
    }
    v
}

pub fn consulta_fts(termos: &str) -> String {
    termos
        .split(|c: char| !c.is_alphanumeric())
        .filter(|p| p.chars().count() >= 2)
        .take(12)
        .map(|p| format!("\"{}\"*", p.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[cfg(test)]
mod testes {
    use super::*;

    fn msg(chat: &str, id: &str, data: i64, texto: &str) -> Mensagem {
        Mensagem {
            chat: chat.into(),
            id: id.into(),
            remetente: "x@s.whatsapp.net".into(),
            nome_remetente: "Fulano".into(),
            de_mim: false,
            data,
            tipo: "texto".into(),
            texto: Some(texto.into()),
            origem: "ao_vivo".into(),
        }
    }

    #[test]
    fn nao_duplica_e_busca_sem_acento() {
        let dir = std::env::temp_dir().join(format!("assist-teste-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let arq = dir.join("t.db");
        let _ = std::fs::remove_file(&arq);
        let b = Banco::abrir(&arq).unwrap();
        b.registrar_conversa("1@s.whatsapp.net", Some("João"), false).unwrap();
        assert!(b.salvar_mensagem(&msg("1@s.whatsapp.net", "A", 100, "Te mando o orçamento depois")).unwrap());
        assert!(!b.salvar_mensagem(&msg("1@s.whatsapp.net", "A", 100, "Te mando o orçamento depois")).unwrap());
        b.salvar_mensagem(&msg("1@s.whatsapp.net", "B", 400000, "segue o orcamento em anexo")).unwrap();
        let r = b.buscar("orcamento", 10).unwrap();
        assert_eq!(r.len(), 2, "deve achar com e sem acento, em dias diferentes");
        assert_eq!(r[0].nome_chat, "João");
        b.definir_texto("1@s.whatsapp.net", "A", "outro texto").unwrap();
        assert_eq!(b.buscar("orcamento", 10).unwrap().len(), 1);
        assert_eq!(b.contadores().mensagens, 2);
        b.apagar_historico().unwrap();
        assert_eq!(b.contadores().mensagens, 0);
    }

    #[test]
    fn termos_da_pergunta() {
        assert_eq!(termos_importantes("! Consegue identifica o grupo condominio jardim ?"), vec!["condominio", "jardim"]);
        assert_eq!(termos_importantes("Quem mandou os PDFs das matérias?"), vec!["pdf", "materia"]);
    }

    #[test]
    fn busca_por_relevancia_e_por_conversa() {
        let b = Banco::abrir(std::path::Path::new(":memory:")).unwrap();
        b.salvar_mensagem(&msg("a@g.us", "1", 100, "de de de de reunião de hoje")).unwrap();
        b.salvar_mensagem(&msg("b@g.us", "2", 50, "o PDF de direito civil está no drive")).unwrap();
        b.salvar_mensagem(&msg("b@g.us", "3", 60, "valeu")).unwrap();
        let t = termos_importantes("tem algum pdf de direito?");
        let r = b.buscar_ranqueado(&t, None, 10).unwrap();
        assert_eq!(r[0].id, "2");
        assert!(b.buscar_ranqueado(&t, Some("a@g.us"), 10).unwrap().is_empty());
    }

    #[test]
    fn consulta_fts_segura() {
        assert_eq!(consulta_fts("a \"b\" OR; drop"), "\"OR\"* OR \"drop\"*");
        assert_eq!(consulta_fts(""), "");
    }
}
