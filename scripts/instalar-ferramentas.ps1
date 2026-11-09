# Baixa as ferramentas de transcrição de áudio (whisper.cpp + modelo em português).
# Uso: clique com o botão direito > "Executar com o PowerShell"
#      ou: powershell -ExecutionPolicy Bypass -File scripts\instalar-ferramentas.ps1

param(
    # Modelos: base (~140 MB, rápido), small (~470 MB, bom), medium (~1,5 GB, melhor e lento)
    # Vazio = escolhe pelo computador (menos de 10 GB de memória: base; senão: small)
    [string]$Modelo = "",
    [string]$VersaoWhisper = "b5454"
)

$ErrorActionPreference = "Stop"
if (-not $Modelo) {
    $ramGB = (Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB
    $Modelo = if ($ramGB -lt 10) { "base" } else { "small" }
    Write-Host ("Memória: {0:N0} GB -> modelo de transcrição '{1}'" -f $ramGB, $Modelo)
}
$ProgressPreference = "SilentlyContinue"

$antiga = Join-Path $env:LOCALAPPDATA "AssistenteWhatsApp"
$dados = if ($env:ASSISTENTE_DADOS) { $env:ASSISTENTE_DADOS }
         elseif ((Test-Path (Join-Path $antiga "config.toml")) -and -not (Test-Path (Join-Path $env:LOCALAPPDATA "Recado"))) { $antiga }
         else { Join-Path $env:LOCALAPPDATA "Recado" }
$pasta = Join-Path $dados "ferramentas"
New-Item -ItemType Directory -Force $pasta | Out-Null

Write-Host "Pasta das ferramentas: $pasta"

# 1) whisper.cpp (programa de transcrição)
$exe = Get-ChildItem $pasta -Recurse -Filter "whisper-cli.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $exe) {
    $url = "https://github.com/ggml-org/whisper.cpp/releases/download/$VersaoWhisper/whisper-bin-x64.zip"
    $zip = Join-Path $pasta "whisper.zip"
    Write-Host "Baixando whisper.cpp ($VersaoWhisper)..."
    Invoke-WebRequest -Uri $url -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath (Join-Path $pasta "whisper") -Force
    Remove-Item $zip
    $exe = Get-ChildItem $pasta -Recurse -Filter "whisper-cli.exe" | Select-Object -First 1
}
Write-Host "whisper-cli: $($exe.FullName)"

# 2) Modelo de reconhecimento de fala (multilíngue, entende português)
$arqModelo = Join-Path $pasta "ggml-$Modelo.bin"
if (-not (Test-Path $arqModelo)) {
    $url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$Modelo.bin"
    Write-Host "Baixando modelo '$Modelo' (pode demorar)..."
    Invoke-WebRequest -Uri $url -OutFile "$arqModelo.parcial"
    Move-Item "$arqModelo.parcial" $arqModelo
}
Write-Host "Modelo: $arqModelo"

# 3) FFmpeg
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    Write-Host "FFmpeg não encontrado. Instalando pelo winget..."
    winget install --id Gyan.FFmpeg -e --scope user --accept-source-agreements --accept-package-agreements
}

Write-Host ""
Write-Host "Pronto! Ferramentas de transcrição instaladas." -ForegroundColor Green
