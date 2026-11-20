<p align="center">
  <a href="https://getartcraft.com/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/brand/artcraft-logo-white.svg">
      <img alt="ArtCraft" src="docs/brand/artcraft-logo.svg" width="200">
    </picture>
  </a>
</p>

<p align="center">
  <img src="assets/app-icon/hicolor/256x256/apps/ai.storyteller.craft-anything.png" width="128" height="128" alt="סמל Craft Anything: עורב חרוט על אריח כחול">
</p>

<h1 align="center">Craft Anything</h1>

<p align="center">
  <a href="README.md">English</a> · <a href="README.ja.md">日本語</a> · <a href="README.zh.md">简体中文</a> · <a href="README.zh-hant.md">繁體中文</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.fr.md">Français</a> · <a href="README.vi.md">Tiếng Việt</a> · <a href="README.pt.md">Português</a> · <a href="README.th.md">ไทย</a> · <a href="README.tr.md">Türkçe</a> · <b>עברית</b>
</p>

<div dir="rtl">

<p align="center">
  <b>ראו כיצד כל אתר, אפליקציה או משחק עובדים, ותנו ל-AI לבנות גרסה משלכם — מנוע קוד פתוח בשיטת חדר נקי (clean-room) שנכתב ב-Rust טהור.</b><br>
  הצביעו על יעד, תארו מה תרצו לבנות, והוא יחקור את המקור באמצעות כלי בדיקה אמיתיים,<br>
  יפעיל סוכן קידוד לבניית פרויקט חדש, ישווה את התוצאה למקור וילטש אותה.
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
  <a href="https://discord.gg/artcraft"><img alt="הצטרפו לקהילת ArtCraft ב-Discord" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<p align="center">
  <a href="#הורדות"><b>הורדת Craft Anything</b></a> ·
  <a href="https://getartcraft.com/">ArtCraft</a> ·
  <a href="https://getartcraft.com/apps">כל אפליקציות Crafting</a>
</p>

<br>

<p align="center">
  <img src="docs/images/project-website.png" alt="פרויקט אתר ב-Craft Anything: הבדיקה מפרטת מה טעון התאמה וכמה כל עמוד דומה למקור, הבונה מודיע שהגרסה מוכנה, כרטיס אישור מבקש אישור לפני הרצת פקודה, והתוכנית מציגה שני שלבים שהושלמו מתוך ארבעה" width="100%">
  <br>
  <sub>פרויקט אתר: כל בדיקה מראה עד כמה הבנייה דומה למקור, אתם מאשרים כל פעולה החורגת מהכללים, והתוכנית מציגה מה נשאר.</sub>
</p>

> [!NOTE]
> **ArtCraft היא קהילה של יוצרים ואמנים מכל התחומים.** ציירים, צלמים,
> קולנוענים, מאיירים, מעצבים, אנימטורים, חובבים וגם מי שהרימו עיפרון
> רק בשבוע שעבר. אם אתם יוצרים דברים — מקומכם איתנו. **[בואו לומר שלום ב-Discord.](https://discord.gg/artcraft)**

<p align="center">
  <a href="#איך-זה-עובד">איך זה עובד</a> ·
  <a href="#שתי-דרכים-לעבוד">שתי דרכים לעבוד</a> ·
  <a href="#ai">AI</a> ·
  <a href="#בנוי-לסוכנים">סוכנים</a> ·
  <a href="#מתחת-למכסה">מתחת למכסה</a> ·
  <a href="#התחלה-מהירה">התחלה מהירה</a> ·
  <a href="#מצב-הפרויקט">סטטוס</a> ·
  <a href="#הורדות">הורדות</a> ·
  <a href="https://discord.gg/artcraft">Discord</a>
</p>

<br>

<table>
  <tr>
    <td width="25%" valign="top">
      <h3>🔍 לחקור כל דבר</h3>
      אתרים, אפליקציות שולחן עבודה, אפליקציות מובייל, משחקים וקבצים: כלי חקירה אמיתיים חושפים איך הם בנויים ומה הם עושים, עם ראיות מתועדות מאחורי כל תשובה.
    </td>
    <td width="25%" valign="top">
      <h3>🛠️ לבנות גרסה משלכם</h3>
      תארו מה אתם רוצים (״לשחזר את האתר הזה״, ״לשחזר את המכניקה של המשחק ב-Rust״), וסוכן קידוד יבנה פרויקט חדש בטכנולוגיה שתבקשו.
    </td>
    <td width="25%" valign="top">
      <h3>✅ נבדק, לא מנוחש</h3>
      כל בנייה מושווית למקור (עמודים בשלושה רוחבי מסך, בדיקה עצמית למשחק או ריצות של אפליקציה) ומשופרת עד להתאמה קרובה.
    </td>
    <td width="25%" valign="top">
      <h3>🤖 מוכן לסוכנים</h3>
      320 כלים — כל אחד מהם הוא גם תת-פקודת CLI וגם כלי MCP, כך שכל סוכן יכול להפעיל את אותו מנוע של האפליקציה. אין צורך ב-AI כדי לחקור.
    </td>
  </tr>
</table>

<br>

## איך זה עובד

הצביעו על יעד ב-Craft Anything, תארו מה תרצו לבנות ולחצו **התחלה**. התהליך עובד בארבעה שלבים, ושואל אתכם בשפה פשוטה כשנדרשת הכרעה שלכם:

1. **חקירה (Study):** בדיקת המקור — עמודים בכל רוחב מסך, קוד, קבצים ואופן הריצה.
2. **בנייה (Build):** סוכן קידוד בונה פרויקט חדש בשפה, בפריימוורק או במנוע שציינתם בבקשה (או ברירת מחדל מתאימה).
3. **בדיקה (Check):** השוואת הבנייה למקור והצגת רשימת הפערים שעוד נותר לסגור.
4. **מוכן (Ready):** פתיחה, הרצה או תצוגה מקדימה של התוצאה, או בקשת שינויים לליטוש נוסף.

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/images/make.png" alt="מסך הבנייה: בחרו אתר, אפליקציה או משחק לשחזור, הזינו כתובת, בחרו Clean room או Deep, תארו מה תרצו ולחצו Start" width="100%"><br>
      <sub><b>בנייה:</b> בחרו מה לשחזר, כתבו מה תרצו ולחצו התחלה.</sub>
    </td>
    <td width="50%" valign="top">
      <img src="docs/images/project-game.png" alt="פרויקט משחק סנייק מוכן עם כל השלבים שהושלמו וכפתור להפעלת המשחק" width="100%"><br>
      <sub><b>מוכן:</b> הפעילו את המשחק, או בקשו שינויים למטה.</sub>
    </td>
  </tr>
</table>

## שתי דרכים לעבוד

- **פשוט (Simple, ברירת מחדל):** ״מה תרצו ליצור?״ בוחרים אתר, אפליקציה או משחק, מתארים בקצרה ולוחצים Start. עמוד הפרויקט מציג את ארבעת השלבים (חקירה ← בנייה ← בדיקה ← מוכן), מה הסוכן עושה במילים ברורות, כרטיסי שאלות ואישורים, ציון ההתאמה למקור וכפתורים לפתיחת התוצר או התיקייה שלו. אפשר לבקש שינויים בתיבה למטה. אפשרות ״להסתכל מבפנים בלבד״ פותחת יעד לתשאול בלי לבנות דבר (במצב Deep).
- **מתקדם (Advanced):** סביבת העבודה המלאה: כל הכלים (Ctrl/Cmd+K), פונקציות, מחרוזות, פסאודו-C, אסמבלי (Disassembly), Hex, גרפי קריאות, ראיות, נעלמים, עבודות, לוגים ומסוף הסוכן.

שני המצבים מפעילים את אותו מנוע.

## חדר נקי (Clean room) או עמוק (Deep)

- **Clean room (ברירת מחדל):** דבר מתוך המקור אינו נקרא — לא הקוד, לא הקבצים ולא קוד המקור של העמוד. Craft Anything עובד רק על סמך מה שניתן לראות: תמונות שהוספתם, האתר כפי שגולש רואה אותו (צילומי מסך, טקסט גלוי, מידות פריסה, צבעים, גופנים, מרווחים ותפריטים פתוחים), וכאשר אתם מאשרים זאת — גם צילום חלון האפליקציה. כלים שמבקשים להציץ פנימה נדחים עם הסבר כיצד להפעיל מצב Deep.
- **Deep:** כל הכלים זמינים לשימושכם הפרטי. קוד התוכנה וקבציה נקראים באמצעות המנועים המובנים (ועם Ghidra אם מותקן), ה-JavaScript של אתר נקרא ועמוד הבית נסרק, והבונה רשאי להשתמש בכל הממצאים.

מצב חדר נקי הוא מדיניות של הכלים ש-Craft Anything מציע ומסרב להם; הוא אינו ארגז חול (sandbox) ברמת מערכת ההפעלה ואינו מבודד את המחשב. סוכן קידוד שאתם מריצים פועל תחת ההרשאות שלכם. האפשרות להגביל את הכלים המובנים של סוכן תלויה בסוכן עצמו; לגבי כלי Claude Code בשיחות ובשאלות, ראו [`docs/claude-code.md`](docs/claude-code.md).

אפשר לעבור בין המצבים בשורת הכותרת או בהגדרות, באמצעות `settings_set deep_mode true`, או להרצת CLI חד-פעמית עם `--deep` (או `--clean-room`).

<p align="center">
  <img src="docs/images/workbench.png" alt="סביבת העבודה המתקדמת: רשימת פונקציות, פסאודו-C של הפונקציה הנבחרת, הסוכן עונה על שאלה באמצעות קריאת כלי, ויומן הראיות" width="100%">
  <br>
  <sub><b>מתקדם:</b> פונקציות, פסאודו-C, הסוכן והראיות מאחורי כל תשובה.</sub>
</p>

## AI

הבנייה דורשת סוכן קידוד מותקן במחשב שלכם. **Claude Code** נתמך כבחירה ראשונה: Craft Anything מריץ את תוכנת `claude` המקורית והלא-משונה שלכם עם החשבון שלכם; Craft Anything לעולם אינו רואה או שומר את פרטי ההתחברות שלכם, והשימוש כפוף להסכם שלכם מול Anthropic (ראו [docs/claude-code.md](docs/claude-code.md)). קיים גם מתאם ל-Codex שטרם נבדק בהתקנה חיה מלאה. **GitHub Copilot** יכול להשתמש ב-Copilot CLI המותקן דרך ACP (התחברות נפרדת עם `copilot login`). **Grok Build** יכול להשתמש ב-`grok` המותקן דרך ACP (`grok agent stdio`; התחברות נפרדת עם `grok login`, ראו [docs/grok.md](docs/grok.md)). סוכנים נוספים התומכים ב-ACP (Gemini CLI, Qwen Code, opencode, goose, Hermes, Maki, Devin) ניתנים לבחירה בכל מנוע קצה; מביניהם Maki ו-Devin הורצו בשידור חי (Devin מזדהה מול מפתח השרת השמור — התחברות CLI מקומית אינה תקפה דרך ACP). בנוסף ניתן לחבר Anthropic, OpenAI, OpenRouter, xAI (Grok, כשמוגדר `XAI_API_KEY`) ושרתים תואמי OpenAI מקומיים לצורך שאלות וניתוח (הגדרות ← AI).

## בנוי לסוכנים

כל כלי הוא גם תת-פקודת CLI וגם כלי MCP, כך שסוכן יכול לחקור יעד או לבצע בנייה מחדש שלמה גם בלי האפליקציה הגרפית:

```sh
craft-anything-cli commands                                   # כל הכלים, לפי ערכת כלים
craft-anything-cli mcp                                        # חשיפתם כשרת MCP לכל לקוח
craft-anything-cli rebuild https://example.com "Recreate this website"
```

התוצאות נשמרות כרשומות ראיות עם רמת ביטחון, כיסוי ומגבלות; פלטים גדולים חוזרים תחילה כתקציר וניתנים לדפדוף עם `core_evidence_browse`. הפקודה `craft-anything-cli setup` מחברת את Craft Anything לאפליקציות סוכני קידוד.

## מתחת למכסה

מנוע הנדסה לאחור ב-Rust טהור עם 320 כלים (300 במנוע והשאר בשכבות ה-AI וההגדרה) לתוכנות נייטיב, ‎.NET, Java, Android, Apple, Electron/JavaScript, אתרים, קושחה, לכידות תעבורה (PCAP), מסדי נתונים וקובצי משחקים. המפענחים, הדיסאסמבלר והדה-קומפילר שלו פועלים ללא תלות חיצונית; מצב Deep משתמש בכולם, וגם ב-Ghidra כשהוא מותקן. אותו מנוע מניע את אפליקציית שולחן העבודה (`egui`), את ה-CLI ואת שרת ה-MCP, וחלקים ממנו רצים גם בדפדפן ב-WebAssembly. הרשימה המלאה זמינה ב-[docs/tools.md](docs/tools.md).

## התחלה מהירה

דורש Rust בגרסה **1.97 ומעלה** (`rustup update`). ב-Debian או Ubuntu אפליקציית שולחן העבודה דורשת גם:

```sh
sudo apt-get install build-essential libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libx11-dev libxrandr-dev libxi-dev libgl1-mesa-dev libgtk-3-dev
```

לאחר מכן:

```sh
git clone https://github.com/storytold/craft-anything && cd craft-anything
cargo run --release -p craft-anything                # אפליקציית שולחן העבודה
cargo install --path apps/craft-anything-cli         # כלי שורת הפקודה (`craft-anything-cli`)
craft-anything-cli doctor                            # בדיקת ההתקנה, הסוכנים והמנועים
```

בנייה דורשת סוכן קידוד מחובר במחשבכם (ראו [AI](#ai)); פקודת `doctor` תפרט מה חסר. הרצה משורת הפקודה (שאלות יופיעו במסוף):

```sh
craft-anything-cli rebuild https://example.com "Recreate this website"
craft-anything-cli rebuild - "A snake game in Rust" --rounds 1
craft-anything-cli rebuild https://example.com "Make the header blue" --continue-in "<project folder>"
craft-anything-cli rebuild - "Add a contact form" --project-folder "<your folder>" --existing edit
```

פרויקטים חדשים נשמרים ב-`Documents/Craft Anything` (ניתן לשינוי בהגדרות ← כללי). כל תיקיית פרויקט מכילה `craft.json` (בקשה, יעד, סטטוס והיסטוריה), `capture/` (חומרי הקלטה מהיעד), `app/` (הפרויקט שנבנה), `checks/` (השוואות למקור), `investigation.cany/` (ראיות) ו-`inbox/` (שאלות פתוחות).

ניתן גם להתחיל פרויקט בתיקייה שתבחרו (״בחר תיקייה…״ תחת התחלה, או `--project-folder`). אם כבר יש בה קבצים, ציינו אם מדובר באפליקציה עצמה לעריכה במקום או בחומר גלם שממנו ייבנה הפרויקט בתוך `app/`.

## מצב הפרויקט

> [!IMPORTANT]
> **סטטוס:** Craft Anything נמצא באלפא מוקדמת. מה כבר עובד, מה חלקי ומה הלאה:
>
> - **עובד היום:** אפליקציית שולחן העבודה (פשוט ומתקדם), ה-CLI ושרת ה-MCP; תהליך חקירה ← בנייה ← בדיקה ← מוכן עבור אתרים, אפליקציות ומשחקים עם Claude Code; בדיקות אתרים בשלושה רוחבי מסך; שחזורי משחקים ב-Unity, Unreal או Godot; מצב Deep עם Ghidra; יצירת תמונות ומודלי תלת-ממד (אופציונלי, עם מפתח fal.ai אישי); תוספים (שרתי MCP ורכיבי WASM עם האפשרות `wasm-plugins`).
> - **חלקי:** בנייה עם מפתחות API וסוכני קידוד אחרים (מוכן בקוד, טרם הורץ מקצה לקצה בסביבה חיה), בדיקות התנהגות לאפליקציות ומשחקים, וחלק מכלי המנוע שנבדקו רק מול דגימות מדומות.
> - **פורסם:** גרסאות אלפא פומביות עם מתקינים ל-Windows, macOS, Linux ו-FreeBSD זמינות ב-[דף השחרורים](https://github.com/storytold/craft-anything/releases).
> - **הבא בתור:** שכבת הענן המנוהלת.
>
> לפרטים מלאים ראו [docs/STATUS.md](docs/STATUS.md), לרשימת הכלים ראו [docs/tool-status.md](docs/tool-status.md), ולמפת הדרכים ראו [docs/ROADMAP.md](docs/ROADMAP.md). נשמח לדיווחי באגים או הערות ב-[Discord](https://discord.gg/artcraft).

**שימוש אחראי.** השתמשו ב-Craft Anything רק בתוכנה שבבעלותכם או שיש לכם רשות לבדוק. הכלי אינו עוקף DRM, מסכי התחברות או מנגנוני אנטי-צ׳יט. שחזורים מפיקים פריסה, סגנון והתנהגות מתוך ראיות; הסוכן מונחה שלא להעתיק סמלילים, תמונות או טקסטים אלא אם אישרתם בעלות או רישיון שימוש.

**התקנת תלויות מריצה קוד.** בעת התקנת תלויות בפרויקט שנבנה, הפקודות רצות על המחשב שלכם ועלולות להריץ סקריפטי התקנה (כגון `npm` או build scripts של Rust ו-Python). Craft Anything מבקש אישור מראש לפני הרצת סוג פקודה חדש בפרויקט.

## הורדות

ב-GitHub, [הגרסה האחרונה](https://github.com/storytold/craft-anything/releases/latest) כוללת את כל החבילות שלהלן, ו-[כל הגרסאות](https://github.com/storytold/craft-anything/releases) זמינות עם הערות שחרור ו-`SHA256SUMS.txt`.

### Windows

| בנייה | מתקין | גרסה ניידת |
|---|---|---|
| x64 (Intel/AMD 64-bit) | `craft-anything-<ver>-windows-x64.msi` | `craft-anything-<ver>-windows-x64-portable.zip` |
| arm64 (ARM / Snapdragon) | `craft-anything-<ver>-windows-arm64.msi` | `craft-anything-<ver>-windows-arm64-portable.zip` |
| x86 (32-bit) | `craft-anything-<ver>-windows-x86.msi` | `craft-anything-<ver>-windows-x86-portable.zip` |

המתקינים וקובצי ההפעלה חתומים דיגיטלית.

### macOS

| בנייה | קובץ | הערות |
|---|---|---|
| אפליקציה אוניברסלית (Apple silicon + Intel) | `craft-anything-<ver>-macos-universal.dmg` | חתומה ומאושרת נוטריזציה |
| כלי שורת הפקודה, אוניברסלי | `craft-anything-cli-<ver>-macos-universal.zip` | חתום ומאושר; מגרסה אחרי 0.2.0 ה-CLI כבר כלול באפליקציה |

### Linux

| פורמט | x86_64 | aarch64 (ARM64) | הערות |
|---|---|---|---|
| AppImage | `craft-anything-<ver>-linux-x86_64.AppImage` | `craft-anything-<ver>-linux-aarch64.AppImage` | רץ מכל מקום; מתעדכן עם AppImageUpdate |
| Flatpak | `craft-anything-<ver>-linux-x86_64.flatpak` | `craft-anything-<ver>-linux-aarch64.flatpak` | רץ בארגז חול (בלי גישה לדפדפן מקומי); `flatpak install --user <file>` |
| Debian/Ubuntu | `craft-anything-<ver>-linux-x86_64.deb` | `craft-anything-<ver>-linux-aarch64.deb` | |
| Fedora/RHEL/openSUSE | `craft-anything-<ver>-linux-x86_64.rpm` | `craft-anything-<ver>-linux-aarch64.rpm` | |
| Tarball | `craft-anything-<ver>-linux-x86_64.tar.gz` | `craft-anything-<ver>-linux-aarch64.tar.gz` | פריסה לכל תיקייה |

### FreeBSD

| בנייה | קובץ |
|---|---|
| x86_64 | `craft-anything-<ver>-freebsd-x86_64.tar.gz` |

### Web (WebAssembly)

| בנייה | קובץ | הערות |
|---|---|---|
| מציג דפדפן | `craft-anything-web-<ver>.zip` | מזהה קבצים בדפדפן מקומית בלי לשלוח אותם לשרת ([packaging/web/README.md](packaging/web/README.md)) |

<br>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="הצטרפו לקהילת ArtCraft ב-Discord" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<h3 align="center">בואו ליצור איתנו דברים</h3>

<p align="center">
  שרת ה-Discord שלנו הוא בית ליוצרים ואמנים מכל הסוגים: ציירים, צלמים, מאיירים, עורכי וידאו, מעצבים ומתחילים סקרנים. שתפו פרויקטים, בקשו עזרה או ספרו מה הייתם רוצים שהכלים האלה ידעו לעשות.
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><b>discord.gg/artcraft</b></a> ·
  <a href="https://getartcraft.com/">getartcraft.com</a> ·
  <a href="https://getartcraft.com/apps">אפליקציות Crafting</a> ·
  <a href="https://github.com/storytold/craft-anything">Craft Anything</a>
</p>

---

## חבילת אפליקציות Craft

| | אפליקציה | למה היא מיועדת | קוד | עוד מידע |
|:-:|---|---|---|---|
| <img src="https://raw.githubusercontent.com/storytold/photocraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.photocraft.png" alt="" width="32" height="32"> | **PhotoCraft** | עריכת תמונות: שכבות, מסכות, טיפוגרפיה וקובצי PSD אמיתיים | [GitHub](https://github.com/storytold/photocraft) | [אתר](https://getartcraft.com/apps/photocraft) |
| <img src="https://raw.githubusercontent.com/storytold/vectorcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.vectorcraft.png" alt="" width="32" height="32"> | **VectorCraft** | איור וקטורי | [GitHub](https://github.com/storytold/vectorcraft) | [אתר](https://getartcraft.com/apps/vectorcraft) |
| <img src="https://raw.githubusercontent.com/storytold/filmcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.filmcraft.png" alt="" width="32" height="32"> | **FilmCraft** | עריכת וידאו, צבע וסאונד | [GitHub](https://github.com/storytold/filmcraft) | [אתר](https://getartcraft.com/apps/filmcraft) |
| <img src="https://raw.githubusercontent.com/storytold/lightcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.lightcraft.png" alt="" width="32" height="32"> | **LightCraft** | ספריית תמונות ופיתוח RAW | [GitHub](https://github.com/storytold/lightcraft) | [אתר](https://getartcraft.com/apps/lightcraft) |
| <img src="https://raw.githubusercontent.com/storytold/pdfcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.pdfcraft.png" alt="" width="32" height="32"> | **PdfCraft** | קריאה, ארגון והגנה על קובצי PDF | [GitHub](https://github.com/storytold/pdfcraft) | [אתר](https://getartcraft.com/apps/pdfcraft) |
| <img src="https://raw.githubusercontent.com/storytold/effectcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.effectcraft.png" alt="" width="32" height="32"> | **EffectCraft** | גרפיקה בתנועה ואפקטים חזותיים | [GitHub](https://github.com/storytold/effectcraft) | [אתר](https://getartcraft.com/apps/effectcraft) |
| <img src="https://raw.githubusercontent.com/storytold/designcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.designcraft.png" alt="" width="32" height="32"> | **DesignCraft** | עימוד והוצאה לאור | [GitHub](https://github.com/storytold/designcraft) | [אתר](https://getartcraft.com/apps/designcraft) |
| <img src="https://raw.githubusercontent.com/storytold/figcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.figcraft.png" alt="" width="32" height="32"> | **FigCraft** | עיצוב ממשקים ואבות-טיפוס | [GitHub](https://github.com/storytold/figcraft) | [אתר](https://getartcraft.com/apps/figcraft) |
| <img src="https://raw.githubusercontent.com/storytold/wordcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.wordcraft.png" alt="" width="32" height="32"> | **WordCraft** | עיבוד תמלילים ומסמכים | [GitHub](https://github.com/storytold/wordcraft) | [אתר](https://getartcraft.com/apps/wordcraft) |
| <img src="https://raw.githubusercontent.com/storytold/soundcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.soundcraft.png" alt="" width="32" height="32"> | **SoundCraft** | הקלטה ועריכת אודיו | [GitHub](https://github.com/storytold/soundcraft) | [אתר](https://getartcraft.com/apps/soundcraft) |
| <img src="https://raw.githubusercontent.com/storytold/deckcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.deckcraft.png" alt="" width="32" height="32"> | **DeckCraft** | מצגות ושקופיות | [GitHub](https://github.com/storytold/deckcraft) | [אתר](https://getartcraft.com/apps/deckcraft) |
| <img src="https://raw.githubusercontent.com/storytold/cadcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.cadcraft.png" alt="" width="32" height="32"> | **CADCraft** | תיב״ם (CAD) | [GitHub](https://github.com/storytold/cadcraft) | [אתר](https://getartcraft.com/apps/cadcraft) |
| <img src="https://raw.githubusercontent.com/storytold/gridcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.gridcraft.png" alt="" width="32" height="32"> | **GridCraft** | גיליונות אלקטרוניים ונתונים | [GitHub](https://github.com/storytold/gridcraft) | [אתר](https://getartcraft.com/apps/gridcraft) |
| <img src="https://raw.githubusercontent.com/storytold/texturecraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.texturecraft.png" alt="" width="32" height="32"> | **TextureCraft** | צביעת טקסטורות בתלת-ממד | [GitHub](https://github.com/storytold/texturecraft) | [אתר](https://getartcraft.com/apps/texturecraft) |
| <img src="https://raw.githubusercontent.com/storytold/materialcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.materialcraft.png" alt="" width="32" height="32"> | **MaterialCraft** | חומרים פרוצדורליים מבוססי צמתים | [GitHub](https://github.com/storytold/materialcraft) | [אתר](https://getartcraft.com/apps/materialcraft) |
| <img src="https://raw.githubusercontent.com/storytold/craft-anything/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.craft-anything.png" alt="" width="32" height="32"> | **Craft Anything** | לראות איך כל תוכנה עובדת ולבנות אחת משלכם | [GitHub](https://github.com/storytold/craft-anything) | [אתר](https://getartcraft.com/apps/craft-anything) |
| <img src="https://raw.githubusercontent.com/tangency-labs/solidcraft/main/assets/icons/app/linux/exports/hicolor/64x64/apps/io.tangencylabs.solidcraft.png" alt="" width="32" height="32"> | **SolidCraft** | תיב״ם תלת-ממד פרמטרי מבית Tangency Labs (אפליקציה קהילתית) | [GitHub](https://github.com/tangency-labs/solidcraft) | |

## רישיון וקרדיטים

Craft Anything מופץ תחת רישיון כפול [MIT](LICENSE-MIT) או [Apache-2.0](LICENSE-APACHE), לפי בחירתכם.
Copyright (c) 2026 ArtCraft Team ותורמי Craft Anything ([CONTRIBUTORS.md](CONTRIBUTORS.md)). הודעות נדרשות נמצאות ב-[NOTICE](NOTICE).

גופנים, סמלים, תמונות ונכסים אחרים שומרים על רישיונות הקוד הפתוח המקוריים שלהם כמפורט ב-[ATTRIBUTION.md](ATTRIBUTION.md).

השם ArtCraft, הסימן המסחרי והלוגואים בתיקייה [`docs/brand/`](docs/brand/) הם סימנים מסחריים של צוות ArtCraft ואינם מכוסים ברישיון זה (ראו [`docs/brand/LICENSE-brand.txt`](docs/brand/LICENSE-brand.txt)).

</div>

<p align="center">
  <a href="https://getartcraft.com/"><img alt="ArtCraft" src="docs/brand/artcraft-mark.svg" width="28"></a><br>
  <sub>נוצר על ידי צוות <a href="https://getartcraft.com/">ArtCraft</a> והקהילה.</sub>
</p>
