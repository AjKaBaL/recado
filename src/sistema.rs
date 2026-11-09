//! Integração com o Windows: ícone perto do relógio (bandeja), iniciar com o
//! Windows e janelas de aviso. Assim o assistente roda sem janela preta.

use crate::app::AppRef;
use anyhow::{Result, anyhow};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tracing::warn;

/// Argumento usado quando o Windows abre o assistente ao ligar: não abre o navegador.
pub const ARG_SEGUNDO_PLANO: &str = "--segundo-plano";

pub fn abrir_navegador(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(0x0800_0000) // sem janela
            .spawn();
    }
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

// ---------------------------------------------------------------- notificações

/// Nome com que o Windows identifica as notificações do assistente.
const ID_NOTIFICACAO: &str = "AjKaBaL.Recado";

/// Mostra uma notificação do Windows (o balão no canto da tela, que também
/// fica na Central de Notificações). Clicar nela abre o painel.
pub fn notificar(pasta: &std::path::Path, titulo: &str, texto: &str, url: &str) -> Result<()> {
    #[cfg(windows)]
    {
        use tauri_winrt_notification::{Duration, Sound, Toast};
        let icone = pasta.join("icone.png");
        if !icone.exists() {
            let _ = std::fs::write(&icone, include_bytes!("../recursos/icone.png"));
        }
        // Registra o nome e o ícone que aparecem na notificação (só na conta do usuário).
        win::registrar_notificacoes(ID_NOTIFICACAO, "Recado", &icone.to_string_lossy());
        let u = url.to_string();
        Toast::new(ID_NOTIFICACAO)
            .title(titulo)
            .text1(texto)
            .sound(Some(Sound::Default))
            .duration(Duration::Short)
            .on_activated(move |_| {
                abrir_navegador(&u);
                Ok(())
            })
            .show()
            .map_err(|e| anyhow!("notificação do Windows falhou: {e:?}"))?;
    }
    #[cfg(not(windows))]
    let _ = (pasta, titulo, texto, url);
    Ok(())
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    const CHAVE: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const NOME: &str = "Recado";
    const NOME_ANTIGO: &str = "AssistenteWhatsApp";

    /// Quem ligou "iniciar com o Windows" na versão com o nome antigo: passa para o nome novo.
    pub fn migrar_nome_antigo() {
        let (c, n) = (w(CHAVE), w(NOME_ANTIGO));
        let existe = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER, c.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ,
                std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(),
            ) == ERROR_SUCCESS
        };
        if existe && definir(true).is_ok() {
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, c.as_ptr(), n.as_ptr()) };
        }
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn iniciar_com_windows() -> bool {
        let (c, n) = (w(CHAVE), w(NOME));
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER, c.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ,
                std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(),
            ) == ERROR_SUCCESS
        }
    }

    pub fn definir(ativar: bool) -> Result<(), String> {
        let (c, n) = (w(CHAVE), w(NOME));
        let r = if ativar {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let valor = w(&format!("\"{}\" {}", exe.display(), super::ARG_SEGUNDO_PLANO));
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER, c.as_ptr(), n.as_ptr(), REG_SZ,
                    valor.as_ptr().cast(), (valor.len() * 2) as u32,
                )
            }
        } else {
            let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, c.as_ptr(), n.as_ptr()) };
            if r == 2 { ERROR_SUCCESS } else { r } // 2 = já não existia
        };
        if r == ERROR_SUCCESS { Ok(()) } else { Err(format!("erro {r} do Windows")) }
    }

    /// Grava no registro (HKCU, Classes, AppUserModelId) o nome e o ícone das notificações.
    pub fn registrar_notificacoes(id: &str, nome: &str, icone: &str) {
        let chave = w(&format!(r"Software\Classes\AppUserModelId\{id}"));
        for (campo, valor) in [("DisplayName", nome), ("IconUri", icone)] {
            let (n, v) = (w(campo), w(valor));
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER, chave.as_ptr(), n.as_ptr(), REG_SZ,
                    v.as_ptr().cast(), (v.len() * 2) as u32,
                );
            }
        }
    }

    pub fn janela_de_erro(texto: &str) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let (t, c) = (w(texto), w("Recado"));
        unsafe {
            MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONERROR);
        }
    }

    /// Abre uma janela de console (para os autotestes). Devolve true se abriu uma nova.
    pub fn abrir_console() -> bool {
        use windows_sys::Win32::System::Console::{AllocConsole, GetConsoleWindow, GetStdHandle, STD_OUTPUT_HANDLE};
        // Se a saída já vai para algum lugar (outro programa, arquivo), não abre janela.
        let saida = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        if !saida.is_null() && saida as isize != -1 {
            return false;
        }
        unsafe { GetConsoleWindow().is_null() && AllocConsole() != 0 }
    }

    /// Processa as mensagens do Windows pendentes (necessário para o ícone da bandeja).
    pub fn processar_mensagens() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
        };
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(windows)]
pub use win::{abrir_console, iniciar_com_windows, janela_de_erro, migrar_nome_antigo};
#[cfg(not(windows))]
pub fn migrar_nome_antigo() {}

#[cfg(windows)]
pub fn definir_iniciar_com_windows(ativar: bool) -> Result<()> {
    win::definir(ativar).map_err(|e| anyhow!(e))
}

#[cfg(not(windows))]
pub fn iniciar_com_windows() -> bool {
    false
}
#[cfg(not(windows))]
pub fn definir_iniciar_com_windows(_: bool) -> Result<()> {
    Err(anyhow!("só disponível no Windows"))
}
#[cfg(not(windows))]
pub fn janela_de_erro(_: &str) {}
#[cfg(not(windows))]
pub fn abrir_console() -> bool {
    false
}

// ---------------------------------------------------------------- bandeja

/// Cria o ícone perto do relógio numa thread própria.
/// Clique: abre o painel. Botão direito: menu (painel, iniciar com o Windows, fechar).
pub fn iniciar_bandeja(app: AppRef, url: String) {
    #[cfg(windows)]
    std::thread::spawn(move || {
        if let Err(e) = rodar_bandeja(&app, &url) {
            warn!("Ícone da bandeja indisponível: {e}");
            app.bandeja_ativa.store(false, Ordering::Relaxed);
        }
    });
    #[cfg(not(windows))]
    let _ = (app, url);
}

#[cfg(windows)]
fn rodar_bandeja(app: &AppRef, url: &str) -> Result<()> {
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let menu = Menu::new();
    let abrir = MenuItem::new("Abrir o painel", true, None);
    let auto = CheckMenuItem::new("Iniciar com o Windows", true, iniciar_com_windows(), None);
    let sair = MenuItem::new("Fechar o assistente", true, None);
    menu.append_items(&[
        &abrir,
        &PredefinedMenuItem::separator(),
        &auto,
        &PredefinedMenuItem::separator(),
        &sair,
    ])
    .map_err(|e| anyhow!("{e}"))?;

    let icone = tray_icon::Icon::from_resource(1, None).map_err(|e| anyhow!("{e}"))?;
    let bandeja = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Recado")
        .with_icon(icone)
        .build()
        .map_err(|e| anyhow!("{e}"))?;
    app.bandeja_ativa.store(true, Ordering::Relaxed);

    let mut ultima_dica = String::new();
    let mut ultima_conferencia = Instant::now() - Duration::from_secs(60);
    loop {
        win::processar_mensagens();

        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == *abrir.id() {
                abrir_navegador(url);
            } else if ev.id == *auto.id() {
                let quer = !iniciar_com_windows();
                if let Err(e) = definir_iniciar_com_windows(quer) {
                    warn!("Não foi possível mudar 'iniciar com o Windows': {e}");
                }
                auto.set_checked(iniciar_com_windows());
            } else if ev.id == *sair.id() {
                app.pedido_sair.store(true, Ordering::Relaxed);
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                abrir_navegador(url);
            }
        }

        if app.pedido_sair.load(Ordering::Relaxed) {
            drop(bandeja); // tira o ícone antes de fechar
            app.sair.notify_one();
            return Ok(());
        }

        if ultima_conferencia.elapsed() > Duration::from_secs(3) {
            ultima_conferencia = Instant::now();
            let e = app.ler_estado();
            let situacao = if e.conectado {
                if app.envio_habilitado() { "conectado, avisos ativos" } else { "conectado, falta !ativar" }
            } else if e.qr.is_some() {
                "esperando ler o QR Code"
            } else {
                "desconectado"
            };
            let dica = format!("Recado — {situacao}");
            if dica != ultima_dica {
                let _ = bandeja.set_tooltip(Some(&dica));
                ultima_dica = dica;
            }
            // O painel também pode mudar esta opção.
            let marcado = iniciar_com_windows();
            if auto.is_checked() != marcado {
                auto.set_checked(marcado);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
