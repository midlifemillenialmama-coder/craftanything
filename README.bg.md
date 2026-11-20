<p align="center">
  <a href="https://getartcraft.com/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/brand/artcraft-logo-white.svg">
      <img alt="ArtCraft" src="docs/brand/artcraft-logo.svg" width="200">
    </picture>
  </a>
</p>

<p align="center">
  <img src="assets/app-icon/hicolor/256x256/apps/ai.storyteller.craft-anything.png" width="128" height="128" alt="Икона на Craft Anything: гравиран гарван върху синя плочка">
</p>

<h1 align="center">Craft Anything</h1>

<p align="center">
  <b>English</b> · <a href="README.ja.md">日本語</a> · <a href="README.zh.md">简体中文</a> · <a href="README.zh-hant.md">繁體中文</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.fr.md">Français</a> · <a href="README.vi.md">Tiếng Việt</a> · <a href="README.pt.md">Português</a> · <a href="README.th.md">ไทย</a> · <a href="README.tr.md">Türkçe</a> · <b>Български</b>
</p>

<p align="center">
  <b>Вижте как работи всеки уебсайт, приложение или игра, след което накарайте AI да изгради ваша собствена версия; двигател с отворен код, изграден на чист Rust.</b><br>
  Насочете го към цел, кажете какво искате и той изучава оригинала с истински инструменти за инспекция,<br>
  кара агент за кодиране да изгради нов проект, проверява резултата спрямо оригинала и го усъвършенства.
</p>

<p align="center">
  <img alt="100% Rust" src="https://img.shields.io/badge/100%25-Rust-b7410e?style=flat-square&logo=rust">
  <img alt="macOS · Windows · Linux · FreeBSD · Web" src="https://img.shields.io/badge/macOS%20%C2%B7%20Windows%20%C2%B7%20Linux%20%C2%B7%20FreeBSD%20%C2%B7%20Web-native-2d81ff?style=flat-square">
  <img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-3a3a3a?style=flat-square">
  <img alt="Status: early alpha" src="https://img.shields.io/badge/status-early%20alpha-d69e2e?style=flat-square">
  <br>
  <a href="https://sonarcloud.io/project/overview?id=storytold_craft-anything"><img alt="Scanned by SonarCloud" src="https://img.shields.io/badge/Scanned_by-SonarCloud-4b9fd5?style=flat-square&logo=sonarqubecloud&logoColor=white"></a>
  <a href="https://coderabbit.ai"><img alt="Reviewed by CodeRabbit" src="https://img.shields.io/coderabbit/prs/github/storytold/craft-anything?style=flat-square&label=Reviewed%20by%20CodeRabbit&logo=coderabbit"></a>
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Присъединете се към общността на ArtCraft в Discord" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<p align="center">
  <a href="#downloads"><b>Изтеглете Craft Anything</b></a> ·
  <a href="https://getartcraft.com/">ArtCraft</a> ·
  <a href="https://getartcraft.com/apps">Всички приложения Craft</a>
</p>

<br>

<p align="center">
  <img src="docs/images/project-website.png" alt="Проект за уебсайт в Craft Anything: проверката изброява какво все още не съвпада и колко си прилича всяка страница, конструкторът казва, че версията му е готова, карта за одобрение пита преди да се изпълни команда, а планът показва две от четири стъпки, изпълнени" width="100%">
  <br>
  <sub>Проект за уебсайт: всяка проверка казва колко си прилича компилацията, вие одобрявате всичко извън правилата, а планът показва какво остава.</sub>
</p>

> [!NOTE]
> **ArtCraft е общност от артисти от всякакви сфери на живота.** Художници, фотографи,
> режисьори, илюстратори, дизайнери, аниматори, любители и хора, които са хванали молив
> миналата седмица. Ако правите неща, вие сте един от нас. **[Елате да се запознаем в Discord](https://discord.gg/artcraft).**

<p align="center">
  <a href="#how-it-works">Как работи</a> ·
  <a href="#two-ways-to-use-it">Два начина за използване</a> ·
  <a href="#ai">AI</a> ·
  <a href="#built-for-agents">Агенти</a> ·
  <a href="#under-the-hood">Под капака</a> ·
  <a href="#get-started">Първи стъпки</a> ·
  <a href="#status">Статус</a> ·
  <a href="#downloads">Изтегляния</a> ·
  <a href="https://discord.gg/artcraft">Discord</a>
</p>

<br>

<table>
  <tr>
    <td width="25%" valign="top">
      <h3>🔍 Изучавайте всичко</h3>
      Уебсайтове, настолни приложения, телефонни приложения, игри и файлове: истински инструменти за инспекция откриват как са изградени и какво правят, с доказателствата зад всеки отговор.
    </td>
    <td width="25%" valign="top">
      <h3>🛠️ Изградете своето</h3>
      Кажете какво искате („Пресъздай този уебсайт“, „Пресъздай геймплея на тази игра в Rust“) и агент за кодиране изгражда нов проект в стека, който поискате.
    </td>
    <td width="25%" valign="top">
      <h3>✅ Проверено, не предположено</h3>
      Всяка компилация се сравнява с оригинала (страници при три ширини на екрана, собствен тест на играта, изпълнения на приложението) и се усъвършенства, докато не стане близо.
    </td>
    <td width="25%" valign="top">
      <h3>🤖 Готово за агенти</h3>
      320 инструмента, всеки от които е CLI команда и MCP инструмент, така че всеки агент може да управлява същия двигател, който използва приложението. Не е необходим AI за инспекция.
    </td>
  </tr>
</table>

<br>

## Как работи

Насочете Craft Anything към цел, кажете какво искате и натиснете Старт. Той работи в четири стъпки,
като ви пита с прости думи, когато решението е ваше:

1. **Проучване:** инспектира оригинала: неговите страници при всяка ширина, неговия код, неговите файлове, как работи.
2. **Изграждане:** агент за кодиране изгражда нов проект на езика, рамката или двигателя, които
   поискате (или разумно по подразбиране).
3. **Проверка:** сравнява компилацията с оригинала и изброява какво все още не съвпада.
4. **Готово:** отворете, пуснете или прегледайте резултата, или поискайте промени и той го усъвършенства.

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/images/make.png" alt="Екранът Make: изберете уебсайт, приложение или игра за пресъздаване, въведете адреса, изберете Clean room или Deep, кажете какво искате и натиснете Start" width="100%"><br>
      <sub><b>Make:</b> изберете какво да пресъздадете, кажете какво искате, натиснете Start.</sub>
    </td>
    <td width="50%" valign="top">
      <img src="docs/images/project-game.png" alt="Завършен проект за игра на змия с всички стъпки изпълнени и бутон Play the game" width="100%"><br>
      <sub><b>Ready:</b> пуснете играта или поискайте промени по-долу.</sub>
    </td>
  </tr>
</table>
## Два начина за използване

- **Опростен** (по подразбиране): „Какво искате да създадете?“ Изберете уебсайт, приложение или игра, опишете го,
  натиснете Старт. Страницата на проекта показва четирите стъпки (Проучване → Изграждане → Проверка → Готово), какво
  прави агентът с прости думи, карти за въпроси и одобрение, колко си прилича резултатът и
  бутони за отваряне на резултата или неговата папка. Поискайте промени в полето в долната част.
  „Просто погледни вътре“ отваря цел за въпроси, без да изгражда нищо (в режим Дълбок).
- **Разширен**: пълната работна маса: всеки инструмент (Ctrl/Cmd+K), функции, низове, псевдо-C,
  дизасемблиране, hex, графики на извикванията, доказателства, неизвестни, задачи, дневници и конзолата на агента.

И двата режима управляват един и същ двигател.

## Чиста стая или Дълбок

- **Чиста стая** (по подразбиране): нищо вътре в оригинала не се чете, нито неговият код, файлове или изходен код на страницата.
  Craft Anything работи с това, което можете да видите: снимки, които добавяте, уебсайтът, както го вижда посетителят
  (екранни снимки, видим текст, измерено оформление, цветове, шрифтове и разстояния, отворени менюта)
  и, когато го позволите, снимка на прозореца на приложението. Инструментите, които биха погледнали вътре, се отказват
  и казват как да включите режим Дълбок.
- **Дълбок**: използва се всеки инструмент, за ваша лична употреба. Кодът и файловете на програмата се четат с
  вградените двигатели (и Ghidra, когато е инсталиран), JavaScript на уебсайта се чете и началната му
  страница се наблюдава, а конструкторът може да използва всички тях.

Чиста стая е правило за това какво Craft Anything предлага и отказва. Това не е операционна система
пясъчник: тя не изолира вашия компютър. Агент за кодиране, който стартирате, все още действа като вас и това, което може да
чете, пише или стартира, зависи от собствените му разрешения. Кои от собствените инструменти на даден агент Craft Anything може
да откаже, се различава според агента; за инструментите на Claude Code в чатове и въпроси вижте
[`docs/claude-code.md`](docs/claude-code.md).

Превключете в заглавната лента или Настройки, с `settings_set deep_mode true`, или за едно CLI изпълнение с
`--deep` (или `--clean-room`).

<p align="center">
  <img src="docs/images/workbench.png" alt="Разширената работна маса: списък с функции, псевдо-C на избраната функция, агентът, който отговаря на въпрос с извикване на инструмент, и регистърът на доказателствата" width="100%">
  <br>
  <sub><b>Разширен:</b> функции, псевдо-C, агентът и доказателствата зад всеки отговор.</sub>
</p>

## AI

Изграждането се нуждае от агент за кодиране на вашия компютър. **Claude Code** се поддържа първи: Craft
Anything стартира вашата собствена, немодифицирана програма `claude`, която използва вашия собствен вход; Craft Anything
никога не вижда и не обработва вашия акаунт. Използването му от ваша страна попада под вашето собствено споразумение с
Anthropic. Вижте [docs/claude-code.md](docs/claude-code.md) за това как и защо. Съществува адаптер за Codex,
но все още не е проверен при реална инсталация. **GitHub Copilot** може да използва вашия инсталиран
Copilot CLI чрез ACP: влезте отделно с `copilot login`. **Grok Build** може да използва вашата
инсталирана програма `grok` чрез ACP (`grok agent stdio`): влезте отделно с `grok login`.
Вижте [docs/grok.md](docs/grok.md). Други агенти, които говорят ACP (Gemini CLI,
Qwen Code, opencode, goose, Hermes, Maki, Devin), могат да бъдат превключени към него за всеки бекенд; от тях Maki
и Devin са били стартирани на живо (Devin се удостоверява със съхранения ключ на бекенда — собственият му CLI
вход не се брои през ACP). Anthropic, OpenAI, OpenRouter, xAI (Grok, когато е зададен `XAI_API_KEY`)
и локални OpenAI-съвместими сървъри са налични за въпроси и анализ (Настройки → AI).

## Създадено за агенти

Всеки инструмент е също CLI подкоманда и MCP инструмент, така че агент може да изучава цел или да стартира
цяло преизграждане без приложението:

```sh
craft-anything-cli commands                                   # всеки инструмент, по набор от инструменти
craft-anything-cli mcp                                        # сервирайте ги на всеки MCP клиент
craft-anything-cli rebuild https://example.com "Пресъздай този уебсайт"
## Първи стъпки

Изисква Rust **1.97 или по-нова версия** (една зависимост го изисква; `rustup update`). В Debian или Ubuntu,
настолното приложение също се нуждае от:

```sh
sudo apt-get install build-essential libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libx11-dev libxrandr-dev libxi-dev libgl1-mesa-dev libgtk-3-dev