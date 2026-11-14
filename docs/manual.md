# 음악 한글발음 링크 찾기

곡 제목으로 한글발음, 노래방 YouTube 영상을 찾는 Windows 프로그램입니다. 첫 번째 결과의 링크 자동 복사를 켜거나, 원하는 검색 결과를 선택해 내 PC에서 VRChat용 임시 MP4 중계를 시작할 수 있습니다.

## 실행

프로젝트 최상위 폴더의 `UtaLink.exe`를 더블클릭하면 됩니다. 별도 실행 스크립트는 필요 없습니다.

기본 창 크기는 1280×900이며 화면의 작업 영역을 넘지 않게 조정합니다. 넓은 창에서는 왼쪽 썸네일 목록과 오른쪽 선택 영상 미리보기, 복사, 접이식 중계 영역으로 표시하고, 작은 창에서는 하단 조작 영역으로 전환합니다. 창 크기는 앱 옆 `window-state.txt`에 저장합니다. 새 레이아웃을 처음 실행할 때는 기본 크기로 시작하고, 이후 직접 조절한 크기를 기억합니다. 창 크기를 저장할 때 중계 설정 파일은 변경하지 않습니다.

- Python 설치 불필요
- 기본 검색은 EXE 하나로 실행 가능
- YouTube API 키 불필요
- 수동 프록싱은 실시간 버퍼 중계를 우선하고, 필요한 경우에만 파일을 준비

## 사용법

1. 곡 제목을 한글, 영문, 일문 중 편한 방식으로 입력합니다.
2. `한글 발음` 또는 `노래방 반주` 모드를 선택합니다.
3. 필요하면 `첫 번째 링크 자동 복사`를 켭니다. **기본값은 꺼짐이며, 검색만으로 프록싱이 시작되지는 않습니다.**
4. Enter 또는 `검색`을 누릅니다.

| 설정 / 버튼 | 동작 |
| --- | --- |
| 자동 복사 꺼짐 (기본) | 검색 결과만 표시합니다. |
| 첫 번째 링크 자동 복사 | 첫 번째 YouTube 결과 링크를 클립보드에 복사합니다. |
| 중계 시작 | 목록에서 선택한 결과의 임시 HTTPS 중계 주소를 준비합니다. 호환 스트림이 있으면 전체 다운로드를 기다리지 않고 재생할 수 있습니다. 첫 번째 결과가 아니어도 됩니다. |
| 중지 | 준비 중인 작업을 취소하고 중계 서버, 터널을 종료합니다. |

목록에서 다른 결과를 선택한 뒤 `링크 복사`를 누르거나 결과를 더블클릭하면 해당 링크가 복사됩니다. `YouTube에서 열기` 버튼으로 선택한 영상을 브라우저에서 바로 열 수도 있습니다.

한 번 클릭하면 영상 선택만 바뀝니다. 목록에 포커스가 있으면 ↑/↓로 이동하고 Enter 또는 Ctrl+C로 선택 링크를 복사할 수 있습니다. 검색어를 입력하거나 한글을 조합하는 동안에는 화살표 키를 가로채지 않습니다. 검색창에서 Ctrl+A로 전체 선택할 수 있습니다. 검색 모드를 바꾸면 현재 검색어로 자동으로 다시 검색합니다.

곡명 일치 결과를 먼저 표시하며, 한글 발음 모드에서는 발음 표기 영상 다음에 `자막` 보조 영상을 표시합니다. 같은 모드의 다른 추천곡은 뒤에 이어집니다. 결과 영역 위에서 마우스 휠을 굴리면 계속 탐색할 수 있으며, 추천곡은 `관련 추천` 표시로 곡명 일치 결과와 구분됩니다. 자막 추천은 `관련 추천 · 자막`으로 표시합니다.

추천은 등록된 전체 아티스트명·별명이나 작품명, 또는 OP/ED·기수와 함께 입력한 전체 작품명을 기준으로 찾습니다. 미등록 일반 곡의 공통 단어만으로 다른 곡을 추천하지 않으며 관련 영상이 없으면 추천 개수를 억지로 채우지 않습니다.

요청한 발음, 노래방 유형에 맞는 곡명 일치 영상이 없으면, 입력한 검색어의 **일반 YouTube 결과**를 원래 순서대로 최대 8개 먼저 보여 줍니다. 자막 영상만 찾은 경우에도 일반 검색으로 보완합니다. 이 영상에는 `일반 결과` 표시가 붙으며, 복사, 열기, 중계 기능은 그대로 사용할 수 있습니다. 이미 찾은 보조 자막과 같은 URL은 자막으로 한 번만 표시합니다. 일반 검색은 곡명·영상 유형·기수 일치를 보장하지 않습니다.

## VRChat용 수동 프록싱 (Windows 데스크톱)

1. 곡을 검색하고, 목록에서 중계할 영상을 클릭한 뒤 `VRChat 중계 펼치기`를 열어 `중계 시작`을 누릅니다. 정확 결과와 추천 결과 모두 선택할 수 있습니다.
2. 앱이 재생 주소 확보와 임시 터널 연결을 동시에 진행합니다. 영상, 음성이 합쳐진 H.264/AAC MP4가 있으면 앞부분과 메타데이터 구간만 버퍼에 받은 뒤 `실시간 중계 준비 완료`를 표시합니다. 플레이어는 전체 다운로드를 기다리지 않고 재생할 수 있습니다.
3. `중계 주소 복사`를 누르고 VRChat 영상 플레이어에 붙여넣습니다. 주소는 `https://임시이름.trycloudflare.com/v/임의토큰/영상ID.mp4` 형태입니다.
4. 재생하는 동안 PC와 앱을 켜 둡니다. **`중계 중지`를 누르거나 앱을 닫으면 중계가 종료되고 기존 주소가 만료됩니다.** 다시 중계를 시작하면 주소가 달라집니다. 다른 곡을 검색하거나 자동 복사 토글을 바꿔도 기존 중계는 유지됩니다.

Cloudflare Quick Tunnel을 사용하므로 계정, 도메인, 포트 포워딩 설정은 필요하지 않습니다. 임시 터널은 가용성이 보장되지 않으며 중국을 포함한 시청자 네트워크에서 실제 접근 가능한지 확인해야 합니다. [Cloudflare Quick Tunnels](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/)

앱을 열거나 검색하는 것만으로는 중계 도구를 내려받거나 터널을 연결하지 않습니다. `중계 시작`을 눌렀을 때 준비를 시작합니다. 준비가 끝나면 중계 링크를 자동 복사하며, 화면의 주소창에서도 직접 복사할 수 있습니다.

전용 도메인을 쓰려면 앱 옆 `config.txt`에 아래 설정을 넣습니다. Cloudflare에서 해당 도메인을 터널의 `http://127.0.0.1:38291` 서비스로 연결해 두어야 합니다. `TUNNEL_TOKEN`이 비어 있으면 임시 주소를 사용합니다. 전용 터널에는 `CUSTOM_DOMAIN`과 1~65535 범위의 `PORT`가 필요합니다.

```ini
CUSTOM_DOMAIN=video.example.com
TUNNEL_TOKEN=
PORT=38291
```

토큰은 프로세스 명령줄 대신 `TUNNEL_TOKEN` 환경변수로 전달합니다. 실제 토큰을 넣은 `config.txt`는 Git 및 Docker 빌드 컨텍스트에서 제외합니다. 실행 파일을 다른 사람에게 전달할 때도 이 파일은 포함하지 마세요. [Cloudflare 터널 실행 옵션](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/run-parameters/)

새 임시 주소의 DNS 게시와 실제 HTTPS 접속을 확인한 뒤 준비 완료를 표시합니다. 너무 이른 조회가 실패로 캐시되는 것을 줄이기 위해 게시 여부를 먼저 확인하며 시스템 DNS 설정은 바꾸지 않습니다. 네트워크에서 이 확인 방식을 지원하지 않으면 일반 DNS, HTTPS 검증을 사용합니다. 터널 제공자나 DNS 등록 지연은 영상 버퍼링과 별개로 초기 준비 시간을 늘릴 수 있습니다.

**빠른 시작을 우선합니다.** 실시간 방식은 YouTube가 제공하는 일체형 MP4의 해상도를 사용하므로, 분리형 720p 영상보다 낮은 화질일 수 있습니다. PC가 YouTube 원본에서 요청 구간을 받아 친구에게 전달하며 친구가 YouTube에 직접 접속하지는 않습니다. 영상마다 256KB 단위로 필요한 구간을 받고 최대 8MB를 공유 버퍼에 보관합니다. 탐색 시에도 앞부분부터 모두 받을 필요 없이 해당 구간을 바로 요청합니다. 한 세션에 최대 16개 영상의 실시간 버퍼를 유지합니다.

호환되는 실시간 스트림이 없거나 원본 구간을 가져올 수 없으면 파일 중계로 전환합니다. 이때 H.264/AAC 트랙은 재인코딩 없이 MP4로 정리하고, 호환되지 않는 트랙만 변환합니다. 분리형 다운로드는 최대 4개 조각을 동시에 받습니다. 파일 방식은 준비 완료까지 기다려야 하며 화면에 `파일 중계 준비 완료`로 구분됩니다.

프록싱에는 FFmpeg와 FFprobe가 필요합니다. `ffmpeg.exe`, `ffprobe.exe`를 앱 옆 `tools` 폴더에 두거나 PATH에 설치하세요. 이 PC에 이미 설치된 FFmpeg도 자동으로 찾습니다. 최신 YouTube 추출에는 Node.js 또는 Deno도 필요할 수 있습니다. Node.js는 PATH 또는 `tools` 폴더에서, Deno는 yt-dlp의 기본 런타임 검색 경로에서 사용합니다. `yt-dlp.exe`와 `cloudflared.exe`는 `tools` 폴더, PATH에서 찾고, 없으면 처음 프록싱할 때 각 프로젝트의 공식 GitHub 릴리스에서 자동 다운로드합니다. 도구 갱신이 필요하면 앱을 닫고 `tools`의 해당 EXE를 새 버전으로 교체하세요. [yt-dlp 공식 안내](https://github.com/yt-dlp/yt-dlp#dependencies), [FFmpeg 다운로드](https://ffmpeg.org/download.html)

현재 최대 720p, 30분 이하, 파일당 1GB 미만의 녹화 영상을 지원하며 라이브는 제외합니다. 파일 중계의 세션 캐시는 최대 4GB이고 앱 옆 `.relay-cache`에서 관리합니다. 정상 종료나 `중계 중지` 시 삭제하며, 비정상 종료로 남은 파일은 앱을 모두 닫은 뒤 이 폴더에서 지울 수 있습니다. `중계 중지`로 준비 중인 작업을 취소하거나 다른 영상을 선택해 `중계 시작`을 누르면 새 선택으로 전환할 수 있습니다. 실시간 원본 주소가 만료되면 `중계 중지` 후 다시 준비하세요.

중계 서버는 로컬 루프백 주소에만 열리고, 터널은 검증된 MP4의 임의 토큰 경로만 공개합니다. GET, HEAD, 단일 바이트 범위 요청을 지원하며 다운로드 제어 API나 PC 파일 목록은 공개하지 않습니다. 실시간 원본은 yt-dlp가 반환한 HTTPS `*.googlevideo.com` 주소만 허용합니다. 주소를 아는 사람은 영상을 재생할 수 있습니다.

VRChat에서 각 시청자는 `Allow Untrusted URLs`를 켜야 합니다. Public/Group Public 인스턴스는 월드 제작자가 해당 도메인을 허용해야 하므로, 매번 도메인이 바뀌는 임시 터널은 Friends/Invite 계열에서 테스트하는 편이 적합합니다. Android/Quest에는 HTTPS가 필요합니다. 실제 VRChat 플레이어 및 친구 측 네트워크 호환성은 별도 확인이 필요합니다. [VRChat 영상 URL 허용 규칙](https://creators.vrchat.com/worlds/udon/video-players/www-whitelist/)

프록싱은 내 PC에서 작동하는 데스크톱 기능이며 Render 웹앱에서 실행되지 않습니다. 자동 복사와 수동 프록싱은 독립적으로 사용할 수 있습니다.

### 검증

```powershell
cargo test --features windows-app --lib --bins
# 실제 YouTube 검색과 일반 결과 대체 확인:
cargo test --lib search::tests::live_ -- --ignored --nocapture
# 실제 네트워크를 사용하는 선택 검사 (임시 터널 생성):
cargo test --features windows-app --lib relay::tests::live_tunnel_serves_a_generated_mp4 -- --ignored --nocapture
cargo test --features windows-app --lib relay::tests::live_youtube_download_and_hosting -- --ignored --nocapture
# 실제 곡으로 전체 다운로드 전 준비 완료, 원격 탐색, 전체 디코딩 확인:
cargo test --features windows-app --lib relay::tests::live_music_buffered_playback -- --ignored --nocapture
# 실시간 스트림이 없는 경우의 파일 준비 경로 확인:
cargo test --features windows-app --lib relay::tests::live_file_fallback -- --ignored --nocapture
```

이 검사는 computer-use 없이 실행합니다. 외부 HTTPS 주소의 HEAD, 부분 요청을 검증하고 FFmpeg로 원격 탐색 재생 및 수신 파일 전체 디코딩을 확인합니다. 시간, 버퍼량, 결과는 `target/relay-benchmark/*-report.json`에 저장합니다. VRChat 앱 자체나 중국 내 네트워크까지 검증하는 검사는 아닙니다.

## 검색 특징

- `잇떼`, `DSCF`, `토키메키 포포롱`, `주문토끼 2기 OP` 같은 별칭, 축약어를 보정합니다.
- 전체 곡명 또는 전체 별칭을 기준으로 일치시키며, 아티스트나 작품명만 맞는 영상은 관련 추천으로 구분합니다.
- 같은 곡과 영상 유형 안에서 커버, 불러보았다, 라이브 공연을 낮게 배치하고 제목의 공식·원곡 표기 단서를 우선합니다. 가수 이름만으로 원곡이라고 추정하지 않습니다.
- 노래방 모드에서는 KY/TJ/금영/태진/카라오케/반주 단서를 우선합니다.
- 요청한 `OP`/`ED`나 기수와 명시적으로 충돌하는 영상은 곡명 일치에서 제외합니다. 작품명으로 찾을 때는 요청한 OP/ED·기수의 명시적 일치가 필요합니다.
- 짧은 별칭을 원제와 연결합니다. 예: `두근어질 → きゅうくらりん`, `잇떼 → 言って。`, `DSCF → 妄想感傷代償連盟`.

영상 유형은 제목의 표기를 기준으로 추정하며, 실제 영상의 발음·자막 내용을 검사하지 않습니다. 곡명 일치·보조 자막은 합계 최대 8개, 추천은 최대 12개이며 일반 검색 보완 시에도 최종 최대 20개를 표시합니다. 상세한 판정·정렬 기준과 검증 방법은 [검색 판정 규칙](search-policy.md)을 참고하세요.

## 구현

Rust 릴리스 최적화와 네이티브 Win32 유니코드 컨트롤로 제작했습니다. 한글 입력은 Windows IME가 직접 처리하며, UI 글꼴은 `Pretendard JP`의 Regular와 SemiBold를 실행 파일에 포함해 한국어와 일본어를 같은 글꼴로 표시합니다. 시스템에 글꼴을 설치하지 않으며, 메모리 등록이 실패하면 `맑은 고딕`으로 대체합니다. 웹도 동일한 글꼴을 로컬에서 제공합니다. 글꼴 라이선스는 `assets/fonts/LICENSE.txt`와 실행 파일 리소스에 포함되어 있습니다. 선택과 호버의 색상 전환은 짧게 끝나며, Windows의 애니메이션 끄기 및 웹의 동작 줄이기 설정을 따릅니다. 썸네일은 16:9 비율을 유지하고 별도 작업자 3개가 용량과 시간 제한 내에서 불러옵니다. 검색이 바뀌면 이전 이미지 요청을 취소하고 임시 이미지를 정리합니다. 미쿠 이미지의 다중 해상도 아이콘이 실행 파일에 내장되며, 탐색기와 창 제목 표시줄 및 작업 표시줄에서 동일하게 사용합니다. 아이콘 원본은 `assets/miku.png`입니다.

Windows 데스크톱 앱을 소스에서 다시 빌드하려면 Rust 도구 모음 설치 후 프로젝트 최상위 폴더에서 다음을 실행합니다. 앱을 종료한 상태에서 실행 파일을 교체하세요.

```powershell
cargo build --release --features windows-app --bin music-link-finder
Copy-Item target/release/music-link-finder.exe UtaLink.exe -Force
```

## 웹앱 (한국어 ,  모바일/데스크톱)

웹앱은 Windows 앱과 **동일한 `src/search.rs`** 로직을 서버에서 호출합니다. 별칭 확장, 점수화, 관련 추천, 일반 검색 대체 및 YouTube HTML 파싱을 공유하며 **API 키나 비밀값이 필요 없습니다.** YouTube HTML 구조 또는 접근 정책 변경에 따라 검색이 실패할 수 있습니다.

### 로컬 실행

Rust 도구 모음(edition 2024 지원) 설치 후:

```bash
cargo run --bin music-link-finder-web
```

브라우저에서 `http://localhost:3000`을 엽니다. 다른 포트는 `PORT=4000 cargo run --bin music-link-finder-web` (PowerShell에서는 `$env:PORT=4000; cargo run --bin music-link-finder-web`)으로 지정합니다. 서버는 `0.0.0.0:$PORT`에 바인딩됩니다.

Windows 빌드 도구 모음에서는 기존 데스크톱 앱도 그대로 빌드할 수 있습니다.

### API

- `GET /api/health` → `{"status":"ok"}` (HTTP 200)
- `GET /api/search?q=잇떼&mode=pronunciation` → `results` 배열 (HTTP 200)
- `mode`: `pronunciation` (기본값), `karaoke`. `q`: 필수, 공백 제외 1~120자.
- 입력 오류: HTTP 400, `{"error":"..."}`. YouTube 통신/파싱 오류: HTTP 502.
- 결과 필드: `title`, `url`, `channel`, `thumbnail_url`, `score`, `matched`, `kind`, `is_recommendation`, `is_fallback`. `kind`는 `pronunciation`, `subtitle`, `karaoke`, `general` 중 하나이며 영상 유형과 추천 여부는 별개입니다. `score`는 신뢰도가 아닌 정렬 키입니다.
- 요청 유형에 맞는 곡명 일치 결과가 있으면 일치 결과 → 보조 자막 → 관련 추천 순서입니다. 없으면 `is_fallback: true`인 일반 YouTube 결과 → 보조 자막 → 관련 추천 순서입니다. 클라이언트는 서버의 순서와 유형을 반영하고 맨 위 결과를 선택·자동 복사합니다.

```bash
cargo fmt --all -- --check
cargo test --bin music-link-finder-web --lib
cargo check --bin music-link-finder-web
```

웹에서는 개별 링크를 복사할 수 있으며, `첫 번째 링크 자동 복사` 스위치를 켜면 검색 완료 시 화면 맨 위 결과의 링크를 자동으로 복사합니다. 스위치는 페이지를 열 때 기본으로 꺼져 있습니다. 브라우저 보안 정책으로 복사가 차단되면 수동 복사 입력창을 보여 줍니다.

HTML은 매번 재검증하며, CSS·JS는 내용 지문이 포함된 `/assets/style.지문.css`, `/assets/app.지문.js` 주소를 사용합니다. 파일 내용이 바뀌면 주소도 바뀌므로 커스텀 도메인의 CDN이나 브라우저에 이전 파일이 남아 있어도 다음 페이지 로드에서는 새 파일을 받습니다. 버전 주소는 장기 캐시를 허용하고, 호환용 `/style.css`, `/app.js` 주소는 재검증하도록 응답합니다. 이전 주소가 이미 CDN에 캐시돼 있다면 그 두 URL만 개별 삭제할 수 있습니다.

### Render 배포 (사용자 실행 단계)

1. 변경 사항을 GitHub 저장소에 푸시합니다.
2. Render 대시보드에서 **New → Blueprint**를 선택하고 저장소를 연결합니다.
3. 루트의 `render.yaml`을 인식하는지 확인한 후 **Apply**합니다. `runtime: docker`, Free 플랜, `/api/health` 헬스 체크가 설정됩니다.
4. 배포 후 Render가 발급한 `https://...onrender.com` 주소에서 검색을 확인합니다. **이 저장소 작업에서는 실제 배포하지 않습니다.**

수동 Web Service를 선택할 경우에도 저장소 루트의 `Dockerfile`을 이용해 **Docker** 런타임으로 배포하면 됩니다. 별도 Build/Start 명령, API 키 또는 환경변수 설정은 필요하지 않습니다. Render는 `PORT`를 주입하며 기본 포트는 `10000`입니다. Docker 시작 명령은 `/usr/local/bin/music-link-finder-web`입니다. 무료 인스턴스는 비활성 시 절전되어 첫 요청이 느릴 수 있습니다.
