# stage.ps1 - Assemble MSIX packaging directory
param(
    [string]$Target = "x64",
    [string]$Version = "1.3.5.0",
    [string]$PublisherDN = ""
)

if (-not $PublisherDN) { $PublisherDN = $env:MSIX_PUBLISHER_DN }

$ErrorActionPreference = "Stop"

# Path configuration (relative to script location)
$ScriptDir = $PSScriptRoot
$ProjectRoot = Split-Path (Split-Path $ScriptDir -Parent) -Parent
switch ($Target) {
    "arm64"  { $RustTarget = "aarch64-pc-windows-msvc" }
    default  { $RustTarget = "x86_64-pc-windows-msvc" }
}
$ReleaseDir = Join-Path $ProjectRoot "src-tauri\target\$RustTarget\release"
$StageDir = Join-Path $ScriptDir "stage"
$AssetsDir = Join-Path $ScriptDir "assets"
$ManifestTemplate = Join-Path $ScriptDir "AppxManifest.xml"

Write-Host "ScriptDir: $ScriptDir" -ForegroundColor Cyan
Write-Host "ReleaseDir: $ReleaseDir" -ForegroundColor Cyan
Write-Host "StageDir: $StageDir" -ForegroundColor Cyan

# Clean old directory
if (Test-Path $StageDir) { Remove-Item $StageDir -Recurse -Force }
New-Item -ItemType Directory -Path $StageDir -Force | Out-Null
New-Item -ItemType Directory -Path "$StageDir\Assets" -Force | Out-Null

# Copy Tauri build output (EXE + DLLs)
Write-Host "Copying build output..." -ForegroundColor Cyan
$exePath = Join-Path $ReleaseDir "PeriTray.exe"
if (-not (Test-Path $exePath)) {
    Write-Error "PeriTray.exe not found at: $exePath"
    exit 1
}
Copy-Item -Path $exePath -Destination $StageDir

# Copy all DLLs (WebView2Loader.dll etc.)
Get-ChildItem -Path $ReleaseDir -Filter "*.dll" | ForEach-Object {
    Copy-Item -Path $_.FullName -Destination $StageDir
}

# 注：**刻意不拷贝前端资源** —— Tauri 在 `cargo build` 期把 `frontendDist`
# （`src-tauri/dist`）编译进 exe，包内不需要 `dist/`。
# 实证：已装 MSIX 包顶层只有 AppxManifest.xml / AppxBlockMap.xml / AppxSignature.p7x /
# AppxMetadata / Assets / PeriTray.exe，界面照常渲染。
# （旧版此处写过 `Join-Path $ProjectRoot "dist"`，路径少了一段 `src-tauri` ⇒ 恒不存在、
#   被 Test-Path 兜住 ⇒ 整块从未执行；即便把路径改对，也只是把已嵌入的前端再塞一份进包。）

# Copy Store icons
Write-Host "Copying Store icons..." -ForegroundColor Cyan
Copy-Item -Path "$AssetsDir\*.png" -Destination "$StageDir\Assets"

# Generate AppxManifest.xml (replace version number and publisher DN)
Write-Host "Generating AppxManifest.xml..." -ForegroundColor Cyan
if (-not $PublisherDN) {
    Write-Error "MSIX_PUBLISHER_DN environment variable is not set. Please set it to your Partner Center publisher CN."
    exit 1
}
$manifest = Get-Content $ManifestTemplate -Raw

# ⚠️ 必须按**结构**替换 Identity 里的 Version，不能匹配某个写死的版本号字面量。
# 旧写法是 `-replace 'Version="1\.3\.5\.0"', ...` —— 与模板里的字面量硬耦合：
# 一旦有人「顺手」把模板的字面量对齐到当前版本，`-replace` **不匹配也不报错**，
# 产出的包就会带着模板里的那个版本号 ⇒ 包身份与 tag 脱钩。
# 而 MSIX 的版本号必须**严格递增**，Store 上传会被拒 —— 且这一步全程静默。
# ⛔ 用 Regex **实例**的 Replace(.., count) 而非静态 `[regex]::Replace(s,p,r,1)`：
# 后者的第 4 个参数是 **RegexOptions**（1 = IgnoreCase），**不是替换次数**，
# 会替换掉**全部**匹配（本仓实测踩过）。
$identityVersionRe = [regex]'(?s)(<Identity\b[^>]*?\bVersion=")[^"]*(")'
if (-not $identityVersionRe.IsMatch($manifest)) {
    Write-Error "AppxManifest.xml 里找不到带 Version 属性的 Identity 元素，模板格式是否变了？"
    exit 1
}
$manifest = $identityVersionRe.Replace($manifest, "`${1}$Version`${2}", 1)

# 占位符用 `.Replace()`（**字面**替换）而非 `-replace`（**正则**替换）：后者的替换串里
# `$$` 会折叠成 `$`、`$&` 会插入整个匹配、`$1` 在模式含分组时会被当成分组引用 ——
# 发布者 DN 一旦含 `$` 就会被静默改坏。占位符替换本就是字面语义，`.Replace()` 才对。
$manifest = $manifest.Replace('__PUBLISHER_DN__', $PublisherDN)
$manifest = $manifest.Replace('__ARCH__', $Target)
$manifest | Set-Content -Path "$StageDir\AppxManifest.xml" -Encoding UTF8

# 自校验：**解析写出的成品**，断言真实 Identity 元素的 Version。
# 刻意不用正则回读 —— 正则可能先命中注释里的示例文本，那样校验的就是错目标；
# 解析 XML 取 `/Package/Identity/@Version` 无歧义，且校验的是**落盘产物**本身。
try {
    [xml]$staged = Get-Content "$StageDir\AppxManifest.xml" -Raw
} catch {
    Write-Error "生成的 AppxManifest.xml 无法解析为 XML：$($_.Exception.Message)"
    exit 1
}
if ($staged.Package.Identity.Version -ne $Version) {
    Write-Error "Identity/@Version 替换失败：期望 $Version，成品里是 $($staged.Package.Identity.Version)。"
    exit 1
}

Write-Host "Stage directory ready: $StageDir" -ForegroundColor Green
