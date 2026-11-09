//! Resumos (diário e por conversa) e respostas a perguntas livres,
//! sempre baseados nas mensagens guardadas.

use crate::analise::REGRAS_SEGURANCA;
use crate::app::{App, agora, data_curta, inicio_de_hoje};
use crate::banco::{Conversa, MensagemExibicao, sem_acento, termos_importantes};
use crate::ia::{Ollama, ProvedorIa, higienizar};
use anyhow::Result;
use chrono::Local;

pub fn provedor(app: &App) -> Ollama {
    Ollama::novo(&app.cfg.ia.url_ollama, &app.cfg.ia.modelo, app.cfg.ia.pensar, app.cfg.ia.contexto)
}

/// Para perguntas: com as mensagens certas já escolhidas, responder sem o modo
/// "pensar" é bem mais rápido neste computador (opção `ia.pensar_nas_perguntas`).
fn provedor_perguntas(app: &App) -> Ollama {
    Ollama::novo(&app.cfg.ia.url_ollama, &app.cfg.ia.modelo, app.cfg.ia.pensar_nas_perguntas, app.cfg.ia.contexto)
}

fn nome_usuario(app: &App) -> String {
    let n = app.ler_estado().nome_conta;
    if n.trim().is_empty() { "o usuário".into() } else { n }
}

fn linha(m: &MensagemExibicao) -> String {
    let quem = if m.de_mim { "Eu" } else if m.nome_remetente.is_empty() { "Alguém" } else { &m.nome_remetente };
    let t = m.texto.clone().unwrap_or_else(|| format!("({})", m.tipo));
    let t: String = higienizar(&t).chars().take(800).collect();
    format!("[{} | {}] {}: {}", higienizar(&m.nome_chat), data_curta(m.data), quem, t)
}

/// Lista de pendências abertas em texto.
pub fn texto_pendencias(app: &App, limite: i64) -> Result<String> {
    let c = app.banco.conexao();
    let mut st = c.prepare(
        "SELECT id, tipo, descricao, prazo_texto, prazo, importancia, chat FROM itens
         WHERE estado='pendente'
         ORDER BY CASE importancia WHEN 'critica' THEN 0 WHEN 'alta' THEN 1 WHEN 'media' THEN 2 ELSE 3 END,
                  prazo IS NULL, prazo, criado DESC
         LIMIT ?1",
    )?;
    let linhas: Vec<(i64, String, String, Option<String>, Option<i64>, String, String)> = st
        .query_map([limite], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
        .collect::<Result<_, _>>()?;
    drop(st);
    drop(c);
    if linhas.is_empty() {
        return Ok("Nenhuma pendência aberta. 🎉".into());
    }
    let mut s = String::new();
    for (id, tipo, d, pt, p, imp, chat) in linhas {
        let marca = match imp.as_str() { "critica" => "🚨", "alta" => "⚠️", "media" => "•", _ => "·" };
        let prazo = match (p, pt) {
            (Some(p), _) => format!(" — até {}", data_curta(p)),
            (None, Some(t)) => format!(" — prazo: {t}"),
            _ => String::new(),
        };
        s.push_str(&format!("{marca} [{id}] {tipo}: {d}{prazo} ({})\n", app.banco.nome_conversa(&chat)));
    }
    Ok(s)
}

/// Resumo do dia. Usa a IA se estiver disponível; senão, uma lista simples.
pub fn resumo_do_dia(app: &App) -> Result<String> {
    let desde = inicio_de_hoje();
    let assuntos: Vec<(String, String, String)> = {
        let c = app.banco.conexao();
        let mut st = c.prepare(
            "SELECT titulo, resumo, estado FROM assuntos WHERE atualizado >= ?1 ORDER BY atualizado DESC LIMIT 25",
        )?;
        st.query_map([desde], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?
    };
    let contagem = app.banco.resumo_contagem(desde)?;
    let pend = texto_pendencias(app, 15)?;

    let mut base = String::new();
    base.push_str("Mensagens recebidas hoje por conversa:\n");
    for (n, q) in &contagem {
        base.push_str(&format!("- {}: {q}\n", higienizar(n)));
    }
    base.push_str("\nAssuntos movimentados hoje:\n");
    for (t, r, e) in &assuntos {
        base.push_str(&format!("- {} [{}]: {}\n", higienizar(t), e, higienizar(r)));
    }
    base.push_str(&format!("\nPendências abertas:\n{pend}"));

    if contagem.is_empty() && assuntos.is_empty() {
        return Ok(format!("📋 *Resumo de hoje*\nNenhuma mensagem nas conversas monitoradas hoje.\n\n*Pendências:*\n{pend}"));
    }
    if !crate::ollama::garantir(app, std::time::Duration::from_secs(90)) {
        return Ok(format!("📋 *Resumo de hoje* (sem IA)\n\n{base}"));
    }
    let sistema = format!(
        "Você é o secretário pessoal de {}. Escreva um resumo do dia para ele/ela ler no WhatsApp: curto, em tópicos, \
em português do Brasil. Comece pelo que exige ação dele/dela. Use só as informações dadas, sem inventar. \
Mantenha os números [N] das pendências.\n\n{REGRAS_SEGURANCA}",
        nome_usuario(app)
    );
    let txt = provedor(app).texto(&sistema, &format!("<contexto>\n{base}\n</contexto>"))?;
    Ok(format!("📋 *Resumo de hoje — {}*\n\n{txt}", Local::now().format("%d/%m")))
}

/// Resumo de uma conversa encontrada pelo nome.
pub fn resumo_conversa(app: &App, nome: &str, dias: i64) -> Result<String> {
    let lista = app.banco.listar_conversas(nome.trim())?;
    let Some(conv) = lista.into_iter().find(|c| c.total_msgs > 0) else {
        return Ok(format!("Não encontrei conversa com \"{nome}\" que tenha mensagens guardadas."));
    };
    let msgs = app.banco.mensagens_periodo(&conv.jid, agora() - dias * 86400, agora(), 150)?;
    if msgs.is_empty() {
        return Ok(format!("Nenhuma mensagem de *{}* nos últimos {dias} dias.", conv.nome));
    }
    if !crate::ollama::garantir(app, std::time::Duration::from_secs(90)) {
        return Ok(format!("Não consegui abrir a IA local (Ollama). Últimas mensagens de *{}*: {}", conv.nome, msgs.len()));
    }
    let mut u = String::from("<mensagens>\n");
    for m in &msgs {
        u.push_str(&linha(m));
        u.push('\n');
    }
    u.push_str("</mensagens>");
    let sistema = format!(
        "Você é o secretário pessoal de {}. (\"Eu\" nas mensagens é ele/ela.) Resuma a conversa abaixo em português: \
assunto principal, o que aconteceu, decisões, o que ficou pendente, o que depende do usuário e datas mencionadas. \
Seja breve e use tópicos. Se algo não estiver claro, diga que não está claro.\n\n{REGRAS_SEGURANCA}",
        nome_usuario(app)
    );
    let txt = provedor(app).texto(&sistema, &u)?;
    Ok(format!("📝 *Resumo — {}* (últimos {dias} dias, {} mensagens)\n\n{txt}", conv.nome, msgs.len()))
}

/// Conversas citadas na pergunta pelo nome (aceita erro de digitação e nome parcial).
/// Devolve as melhores, com quantas palavras do nome bateram.
pub fn conversas_citadas(app: &App, termos: &[String]) -> Vec<(Conversa, Vec<String>)> {
    const IGNORAR_NO_NOME: &[&str] = &["grupo", "do", "da", "de", "dos", "das", "em", "e", "o", "a", "the"];
    let Ok(lista) = app.banco.listar_conversas("") else { return vec![] };
    let mut melhores: Vec<(usize, f32, Conversa, Vec<String>)> = Vec::new();
    for c in lista.into_iter().filter(|c| c.total_msgs > 0 && !c.nome.trim().is_empty()) {
        let nome = sem_acento(&c.nome);
        let palavras: Vec<&str> = nome
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|p| p.chars().count() >= 3 && !IGNORAR_NO_NOME.contains(p))
            .collect();
        if palavras.is_empty() {
            continue;
        }
        let usados: Vec<String> = termos
            .iter()
            .filter(|t| palavras.iter().any(|p| parecidas(t, p)))
            .cloned()
            .collect();
        let acertos = palavras.iter().filter(|p| termos.iter().any(|t| parecidas(t, p))).count();
        if acertos == 0 {
            continue;
        }
        let fracao = acertos as f32 / palavras.len() as f32;
        // Aceita: 2+ palavras do nome, ou o nome inteiro (ex.: contato "Maria"), com palavra longa.
        let forte = acertos >= 2 || (fracao >= 0.99 && palavras.iter().any(|p| p.chars().count() >= 4));
        if forte {
            melhores.push((acertos, fracao, c, usados));
        }
    }
    melhores.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.total_cmp(&a.1)).then(b.2.total_msgs.cmp(&a.2.total_msgs)));
    let Some(topo) = melhores.first().map(|m| (m.0, m.1)) else { return vec![] };
    melhores
        .into_iter()
        .filter(|m| m.0 == topo.0 && (m.1 - topo.1).abs() < 0.01)
        .take(2)
        .map(|m| (m.2, m.3))
        .collect()
}

/// Mesma palavra, prefixo (4+ letras) ou 1 letra de diferença (5+ letras): "condominoi" ≈ "condominio".
fn parecidas(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (na, nb) = (a.chars().count(), b.chars().count());
    if na >= 4 && nb >= 4 && (a.starts_with(b) || b.starts_with(a)) {
        return true;
    }
    na >= 5 && nb >= 5 && na.abs_diff(nb) <= 1 && distancia(a, b) <= 1
}

fn distancia(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut ant: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut atual = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let custo = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            atual[j] = (ant[j] + 1).min(atual[j - 1] + 1).min(ant[j - 1] + custo);
        }
        ant = atual;
    }
    ant[b.len()]
}

fn data_com_ano(ts: i64) -> String {
    use chrono::TimeZone;
    Local.timestamp_opt(ts, 0).single().map(|d| d.format("%d/%m/%Y").to_string()).unwrap_or_default()
}

/// Responde uma pergunta livre usando as mensagens guardadas.
pub fn responder_pergunta(app: &App, pergunta: &str) -> Result<String> {
    if !crate::ollama::garantir(app, std::time::Duration::from_secs(90)) {
        return Ok("Tentei abrir a IA local (Ollama) e ela não respondeu. Confira se o Ollama está instalado; enquanto isso, use !buscar <palavras>.".into());
    }
    let termos = termos_importantes(pergunta);
    let citadas = conversas_citadas(app, &termos);
    // Palavras do nome da conversa não servem para buscar o conteúdo dela.
    let conteudo: Vec<String> = termos
        .iter()
        .filter(|t| !citadas.iter().any(|(_, usados)| usados.contains(t)))
        .cloned()
        .collect();

    let mut trechos: Vec<MensagemExibicao> = Vec::new();
    let mut sobre = String::new();
    let juntar = |v: &mut Vec<MensagemExibicao>, novas: Vec<MensagemExibicao>| {
        for m in novas {
            if !v.iter().any(|x| x.chat == m.chat && x.id == m.id) {
                v.push(m);
            }
        }
    };
    // Pergunta sobre arquivos: a lista de documentos ajuda mais que a busca por palavras.
    let sobre_arquivos = conteudo.iter().any(|t| {
        ["pdf", "arquivo", "documento", "doc", "docx", "material", "apostila", "slide", "planilha"]
            .iter()
            .any(|a| t.starts_with(a))
    });
    if sobre_arquivos {
        if citadas.is_empty() {
            juntar(&mut trechos, app.banco.documentos(None, 40)?);
        } else {
            for (c, _) in &citadas {
                juntar(&mut trechos, app.banco.documentos(Some(&c.jid), 40)?);
            }
        }
    }
    if citadas.is_empty() {
        juntar(&mut trechos, app.banco.buscar_ranqueado(&conteudo, None, 45)?);
        if trechos.len() < 15 {
            juntar(&mut trechos, app.banco.ultimas_mensagens(None, 15)?);
        }
    } else {
        for (c, _) in &citadas {
            let ultimas = c.ultima_msg.map(data_com_ano).unwrap_or_default();
            sobre.push_str(&format!(
                "- \"{}\" ({}): {} mensagens guardadas, de {} até {}.\n",
                higienizar(&c.nome),
                if c.grupo { "grupo" } else { "contato" },
                c.total_msgs,
                c.primeira_msg.map(data_com_ano).unwrap_or_default(),
                ultimas,
            ));
            juntar(&mut trechos, app.banco.buscar_ranqueado(&conteudo, Some(&c.jid), 35)?);
            juntar(&mut trechos, app.banco.ultimas_mensagens(Some(&c.jid), 25)?);
        }
    }
    // Cabe no limite da IA: as mais relevantes primeiro (a ordem acima), até ~12 mil letras.
    let mut total = 0usize;
    trechos.retain(|m| {
        total += linha(m).chars().count() + 1;
        total <= 9_000
    });
    trechos.sort_by_key(|m| m.data);

    let pend = texto_pendencias(app, 15)?;
    let mut u = String::from("<contexto>\n");
    if !sobre.is_empty() {
        u.push_str("CONVERSA ENCONTRADA. O assistente identificou e acompanha esta(s) conversa(s) citada(s) na pergunta:\n");
        u.push_str(&sobre);
        u.push_str(
            "As mensagens abaixo são desta(s) conversa(s). Se a pergunta for se você identifica/encontra a conversa, \
responda que SIM, diga o nome, quantas mensagens estão guardadas e o período, e resuma em uma frase do que ela trata.\n\n",
        );
    } else {
        let nomes: Vec<String> = app
            .banco
            .listar_conversas("")?
            .into_iter()
            .filter(|c| c.total_msgs > 0 && !c.nome.trim().is_empty())
            .take(40)
            .map(|c| higienizar(&c.nome))
            .collect();
        u.push_str(&format!("Algumas conversas acompanhadas: {}\n\n", nomes.join("; ")));
    }
    u.push_str(&format!("Pendências abertas:\n{pend}\n</contexto>\n\n<mensagens>\n"));
    for m in &trechos {
        u.push_str(&linha(m));
        u.push('\n');
    }
    u.push_str(&format!("</mensagens>\n\nPergunta do usuário: {}", higienizar(pergunta)));
    let sistema = format!(
        "Você é o secretário pessoal de {}. Fale diretamente com ele, em primeira pessoa \
(\"Encontrei…\", \"Não encontrei…\"). Cada mensagem vem no formato [Conversa | data] Pessoa: texto. \
\"Eu\" é o próprio usuário; quando outra pessoa escreve \"você\", está falando com alguém da conversa. \
Responda usando SOMENTE o contexto e as mensagens fornecidos. Se a pergunta citar uma conversa, use as \
informações dela no contexto (quantidade de mensagens, período) e o que foi falado nas mensagens dela. \
Cite a conversa e a data das mensagens usadas, ex.: (Grupo X, 07/11 14:32). \
Se a resposta não estiver nas mensagens, diga que não encontrou e sugira uma palavra para !buscar. \
Responda em português, de forma curta.\n\n{REGRAS_SEGURANCA}\nAgora é {}.",
        nome_usuario(app),
        Local::now().format("%d/%m/%Y %H:%M")
    );
    provedor_perguntas(app).texto(&sistema, &u)
}
