//! Autotestes que o usuário pode rodar para conferir as peças locais
//! sem precisar do WhatsApp:
//!   recado.exe --testar-audio caminho\do\audio.ogg
//!   recado.exe --testar-ia
//!
//! O teste de IA usa uma conversa de EXEMPLO (claramente marcada) num banco
//! temporário separado, que é apagado no final. Nada vai para o banco real.

use crate::app::{App, EstadoConexao, agora};
use crate::banco::{Banco, Mensagem};
use crate::config::Config;
use crate::{analise, resumos, transcricao};
use crate::ia::ProvedorIa;
use anyhow::Result;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::RwLock;
use tokio::sync::mpsc;

fn app_temporario(cfg: Config, pasta_ferramentas: &Path) -> Result<(App, std::path::PathBuf)> {
    let dir = std::env::temp_dir().join(format!("assistente-autoteste-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    // Usa as ferramentas instaladas na pasta real.
    let _ = std::fs::remove_dir_all(dir.join("ferramentas"));
    let banco = Banco::abrir(&dir.join("teste.db"))?;
    let (tx, _rx) = mpsc::channel(1);
    let app = App {
        pasta: pasta_ferramentas.to_path_buf(),
        cfg,
        banco,
        estado: RwLock::new(EstadoConexao { nome_conta: "Você".into(), ..Default::default() }),
        conexao: RwLock::new(None),
        fila_audio: tx,
        ia_disponivel: AtomicBool::new(true),
        transcricao_disponivel: AtomicBool::new(false),
        sair: tokio::sync::Notify::new(),
        pedido_sair: AtomicBool::new(false),
        bandeja_ativa: AtomicBool::new(false),
        historico_recebido: std::sync::atomic::AtomicI64::new(0),
        segundo: RwLock::new(None),
        estado_segundo: RwLock::new(Default::default()),
        mudou_segundo: tokio::sync::Notify::new(),
        computador: Default::default(),
        ajuste: None,
    };
    Ok((app, dir))
}

pub fn testar_audio(pasta: &Path, cfg: Config, arquivo: &Path) -> Result<()> {
    let (app, dir) = app_temporario(cfg, pasta)?;
    let f = transcricao::localizar(&app)?;
    println!("FFmpeg:  {}", f.ffmpeg.display());
    println!("Whisper: {}", f.whisper.display());
    println!("Modelo:  {}", f.modelo.display());
    // Copia para não apagar/alterar o original.
    let copia = dir.join(format!("audio.{}", arquivo.extension().and_then(|e| e.to_str()).unwrap_or("ogg")));
    std::fs::copy(arquivo, &copia)?;
    let ini = std::time::Instant::now();
    let r = transcricao::transcrever(&f, &copia, app.cfg.transcricao.threads);
    let _ = std::fs::remove_dir_all(&dir);
    let texto = r?;
    println!("\nTranscrição ({:.1}s):\n{texto}\n", ini.elapsed().as_secs_f32());
    Ok(())
}

pub fn testar_ia(pasta: &Path, cfg: Config) -> Result<()> {
    let (app, dir) = app_temporario(cfg, pasta)?;
    let r = (|| -> Result<()> {
        let ia = resumos::provedor(&app);
        println!("Verificando Ollama ({}, modelo {})...", app.cfg.ia.url_ollama, app.cfg.ia.modelo);
        ia.disponivel()?;

        let chat = "exemplo-grupo@g.us";
        app.banco.registrar_conversa(chat, Some("[EXEMPLO] Grupo da Faculdade"), true)?;
        let ontem = agora() - 86400 - 3600;
        let m = |id: &str, quem: &str, de_mim: bool, data: i64, texto: &str| Mensagem {
            chat: chat.into(), id: id.into(), remetente: quem.into(), nome_remetente: quem.into(),
            de_mim, data, tipo: "texto".into(), texto: Some(texto.into()), origem: "ao_vivo".into(),
        };
        // Dia 1
        for msg in [
            m("E1", "Maria", false, ontem, "Pessoal, o trabalho de sociologia tem que ser entregue até a próxima terça"),
            m("E2", "João", false, ontem + 60, "Eu faço a introdução"),
            m("E3", "Maria", false, ontem + 120, "Eu pego a pesquisa. Você fica com a conclusão, pode ser?"),
            m("E4", "Eu", true, ontem + 180, "Pode, fico com a conclusão"),
            m("E5", "Maria", false, ontem + 240, "Ah, e não esquece de me mandar o PDF do artigo até sexta"),
        ] {
            app.banco.salvar_mensagem(&msg)?;
        }
        println!("\n--- Analisando o DIA 1 (exemplo) ---");
        let r1 = analise::analisar_conversa(&app, &ia as &dyn ProvedorIa, chat, "Você")?;
        imprimir(&app, &r1)?;

        // Dia 2: retoma o assunto sem repetir o contexto.
        let hoje = agora() - 120;
        for msg in [
            m("E6", "Eu", true, hoje, "Maria, segue o PDF do artigo"),
            m("E7", "Maria", false, hoje + 30, "Recebi, valeu! Mudança: a apresentação passou para quarta às 10h"),
        ] {
            app.banco.salvar_mensagem(&msg)?;
        }
        println!("\n--- Analisando o DIA 2 (exemplo; deve ligar ao assunto do dia anterior) ---");
        let r2 = analise::analisar_conversa(&app, &ia as &dyn ProvedorIa, chat, "Você")?;
        imprimir(&app, &r2)?;

        println!("\n--- Pergunta: \"quem ficou com a conclusão?\" ---");
        println!("{}", resumos::responder_pergunta(&app, "quem ficou responsável pela conclusão do trabalho?")?);
        Ok(())
    })();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
    r
}

fn imprimir(app: &App, r: &analise::Resultado) -> Result<()> {
    let c = app.banco.conexao();
    println!("Assuntos:");
    let mut st = c.prepare("SELECT id, titulo, estado, resumo FROM assuntos ORDER BY id")?;
    for l in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))? {
        let (id, t, e, res) = l?;
        println!("  #{id} [{e}] {t}\n      {res}");
    }
    println!("Itens:");
    let mut st = c.prepare("SELECT id, tipo, descricao, COALESCE(prazo_texto,''), importancia, estado FROM itens ORDER BY id")?;
    for l in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?)))? {
        let (id, t, d, p, i, e) = l?;
        println!("  [{id}] {t} ({i}, {e}) {d} {}", if p.is_empty() { String::new() } else { format!("— prazo: {p}") });
    }
    match &r.alerta {
        Some(a) => println!("Alerta que seria enviado:\n{a}"),
        None => println!("Sem alerta."),
    }
    Ok(())
}

pub fn executar(args: &[String], pasta: &Path, cfg: Config) -> Option<Result<()>> {
    match args.get(1).map(String::as_str) {
        Some("--testar-audio") => Some(match args.get(2) {
            Some(a) => testar_audio(pasta, cfg, Path::new(a)),
            None => Err(anyhow::anyhow!("informe o arquivo: --testar-audio audio.ogg")),
        }),
        Some("--testar-ia") => Some(testar_ia(pasta, cfg)),
        Some("--perguntar") => Some(perguntar(pasta, cfg, &args[2..].join(" "))),
        Some("--testar-espaco") => {
            let n = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100_000);
            Some(testar_espaco(n))
        }
        Some(outro) if outro.starts_with("--") => Some(Err(anyhow::anyhow!("opção desconhecida: {outro}"))),
        _ => None,
    }
}

/// Mede quanto espaço em disco as mensagens ocupam, com mensagens de EXEMPLO
/// parecidas com as reais (tamanhos variados, vários grupos e contatos, áudios transcritos).
pub fn testar_espaco(quantidade: usize) -> Result<()> {
    let dir = std::env::temp_dir().join(format!("assistente-espaco-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let arquivo = dir.join("teste.db");
    let banco = Banco::abrir(&arquivo)?;

    const PALAVRAS: &[&str] = &[
        "oi", "bom", "dia", "pessoal", "amanhã", "reunião", "às", "10h", "trabalho", "faculdade", "pode",
        "mandar", "o", "a", "orçamento", "documento", "pdf", "até", "sexta", "feira", "valeu", "obrigado",
        "kkkk", "beleza", "combinado", "preciso", "que", "você", "veja", "isso", "hoje", "depois", "almoço",
        "pagamento", "boleto", "vence", "semana", "que", "vem", "projeto", "entrega", "prova", "aula", "não",
        "sim", "talvez", "lembra", "de", "levar", "chave", "casa", "mãe", "pai", "grupo", "foto", "vídeo",
        "endereço", "rua", "número", "horário", "mudou", "para", "as", "15h", "confirmado", "show",
    ];
    let mut semente: u64 = 42;
    let mut aleatorio = |max: u64| {
        semente = semente.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (semente >> 33) % max
    };
    let chats: Vec<(String, bool)> = (0..40)
        .map(|i| {
            if i < 15 {
                (format!("1203630{:011}@g.us", 4_000_000_000u64 + i), true)
            } else {
                (format!("{:015}@lid", 100_000_000_000_000u64 + i * 7919), false)
            }
        })
        .collect();
    let inicio = agora() - 365 * 86400;
    let mut lote = Vec::with_capacity(1000);
    let t0 = std::time::Instant::now();
    for n in 0..quantidade {
        let (chat, grupo) = &chats[aleatorio(chats.len() as u64) as usize];
        let sorte = aleatorio(100);
        // ~8% áudios (transcrição longa), ~10% mídia sem texto, resto texto curto/médio
        let (tipo, palavras) = match sorte {
            0..=7 => ("audio", 25 + aleatorio(60)),
            8..=17 => ("imagem", if aleatorio(3) == 0 { 4 } else { 0 }),
            _ => {
                let limite = if aleatorio(10) == 0 { 60 } else { 14 };
                ("texto", 2 + aleatorio(limite))
            }
        };
        let texto = (palavras > 0).then(|| {
            (0..palavras).map(|_| PALAVRAS[aleatorio(PALAVRAS.len() as u64) as usize]).collect::<Vec<_>>().join(" ")
        });
        let de_mim = aleatorio(4) == 0;
        let remetente = if de_mim {
            "eu".to_string()
        } else if *grupo {
            format!("{:015}@lid", 200_000_000_000_000u64 + aleatorio(300))
        } else {
            chat.clone()
        };
        lote.push(Mensagem {
            chat: chat.clone(),
            id: format!("3EB0{:016X}", (n as u64).wrapping_mul(0x9E3779B97F4A7C15)),
            remetente,
            nome_remetente: if de_mim { "Eu".into() } else { format!("Pessoa {}", aleatorio(300)) },
            de_mim,
            data: inicio + (n as i64 * 365 * 86400 / quantidade as i64),
            tipo: tipo.into(),
            texto,
            origem: "ao_vivo".into(),
        });
        if lote.len() == 1000 {
            banco.salvar_lote(&lote)?;
            lote.clear();
        }
    }
    banco.salvar_lote(&lote)?;
    let tempo = t0.elapsed();
    drop(banco);

    let tamanho: u64 = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum();
    let por_msg = tamanho as f64 / quantidade as f64;
    let mb = |b: f64| b / 1_048_576.0;
    println!("Mensagens de exemplo gravadas: {quantidade} (em {:.1} s)", tempo.as_secs_f64());
    println!("Espaço total do banco: {:.1} MB", mb(tamanho as f64));
    println!("Média por mensagem: {:.0} bytes (texto + busca + índices)", por_msg);
    for (rotulo, por_dia) in [("pouco uso", 200.0), ("uso médio", 1000.0), ("uso intenso", 5000.0)] {
        println!(
            "  {rotulo:<12} ({por_dia:>5} msgs/dia): ~{:.0} MB por ano",
            mb(por_msg * por_dia * 365.0)
        );
    }
    if std::env::var_os("ASSISTENTE_MANTER_TESTE").is_some() {
        println!("Banco mantido em {}", arquivo.display());
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Ok(())
}

/// Faz uma pergunta usando as mensagens REAIS guardadas, sem enviar nada pelo WhatsApp.
pub fn perguntar(pasta: &Path, cfg: Config, pergunta: &str) -> Result<()> {
    let banco = Banco::abrir(&pasta.join("assistente.db"))?;
    let (tx, _rx) = mpsc::channel(1);
    let app = App {
        pasta: pasta.to_path_buf(),
        cfg,
        banco,
        estado: RwLock::new(EstadoConexao::default()),
        conexao: RwLock::new(None),
        fila_audio: tx,
        ia_disponivel: AtomicBool::new(true),
        transcricao_disponivel: AtomicBool::new(false),
        sair: tokio::sync::Notify::new(),
        pedido_sair: AtomicBool::new(false),
        bandeja_ativa: AtomicBool::new(false),
        historico_recebido: std::sync::atomic::AtomicI64::new(0),
        segundo: RwLock::new(None),
        estado_segundo: RwLock::new(Default::default()),
        mudou_segundo: tokio::sync::Notify::new(),
        computador: Default::default(),
        ajuste: None,
    };
    let termos = crate::banco::termos_importantes(pergunta);
    let citadas = resumos::conversas_citadas(&app, &termos);
    println!("Palavras usadas: {termos:?}");
    println!("Conversas reconhecidas: {:?}", citadas.iter().map(|c| c.0.nome.as_str()).collect::<Vec<_>>());
    let t0 = std::time::Instant::now();
    let r = resumos::responder_pergunta(&app, pergunta)?;
    println!("
Resposta ({:.0} s):
{r}", t0.elapsed().as_secs_f64());
    Ok(())
}
