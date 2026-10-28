# 음악 한글발음 링크 찾기

곡 제목으로 한글발음·노래방 YouTube 영상을 찾는 Windows 프로그램입니다. 첫 번째 결과의 링크 자동 복사 또는 내 PC를 이용한 VRChat용 임시 MP4 중계를 선택할 수 있습니다.

## 실행

`MusicLinkFinder.exe` 또는 `run.bat`을 더블클릭하면 됩니다.

- Python 설치 불필요
- 기본 검색은 EXE 하나로 실행 가능
- YouTube API 키 불필요
- 자동 프록싱은 영상을 임시 다운로드한 뒤 MP4로 중계

## 사용법

1. 곡 제목을 한글·영문·일문 중 편한 방식으로 입력합니다.
2. `한글발음 영상` 또는 `노래방 · 카라오케` 모드를 선택합니다.
3. 필요한 자동 동작을 켭니다. **자동 프록싱과 자동 복사는 동시에 켤 수 없으며, 앱을 시작하면 둘 다 꺼져 있습니다.** 한쪽을 켜면 다른 쪽은 꺼집니다. 켜진 쪽을 다시 누르면 둘 다 꺼집니다.
4. Enter 또는 `영상 검색`을 누릅니다.

| 설정 | 검색 완료 후 동작 |
| --- | --- |
| 둘 다 꺼짐 (기본) | 결과만 표시합니다. |
| 첫 번째 링크 자동 복사 | 첫 번째 YouTube 결과 링크를 클립보드에 복사합니다. |
| 첫 번째 영상 자동 프록싱 | 첫 번째 결과를 다운로드·MP4 변환하고 임시 HTTPS 중계 주소를 표시합니다. `중계 링크 복사`로 복사합니다. |

목록에서 다른 결과를 선택한 뒤 `선택 링크 복사`를 누르거나 결과를 더블클릭하면 해당 링크가 복사됩니다. `유튜브에서 열기` 버튼으로 선택한 영상을 브라우저에서 바로 열 수도 있습니다.

정확한 검색 결과 다음에는 같은 모드의 다른 추천곡이 이어집니다. 결과 영역 위에서 마우스 휠을 굴리면 계속 탐색할 수 있으며, 추천곡은 초록색 `추천` 표시로 정확 결과와 구분됩니다.

## VRChat용 자동 프록싱 (Windows 데스크톱)

1. `첫 번째 영상 자동 프록싱`을 켜고 곡을 검색합니다.
2. 앱이 영상을 다운로드하고 H.264/AAC MP4로 변환합니다. 변환을 마치고 외부 접속까지 확인한 뒤에만 중계 완료를 표시합니다.
3. `중계 링크 복사`를 누르고 VRChat 영상 플레이어에 붙여넣습니다. 주소는 `https://임시이름.trycloudflare.com/v/임의토큰/영상ID.mp4` 형태입니다.
4. 재생하는 동안 PC와 앱을 켜 둡니다. **프록싱을 끄거나 자동 복사로 전환하거나 앱을 닫으면 중계가 종료되고 기존 주소가 만료됩니다.** 다음에 프록싱을 켜면 주소가 달라집니다.

Cloudflare Quick Tunnel을 사용하므로 계정·도메인·포트 포워딩 설정은 필요하지 않습니다. 임시 터널은 가용성이 보장되지 않으며 중국을 포함한 시청자 네트워크에서 실제 접근 가능한지 확인해야 합니다. [Cloudflare Quick Tunnels](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/)

프록싱에는 FFmpeg와 FFprobe가 필요합니다. `ffmpeg.exe`, `ffprobe.exe`를 앱 옆 `tools` 폴더에 두거나 PATH에 설치하세요. 이 PC에 이미 설치된 FFmpeg도 자동으로 찾습니다. 최신 YouTube 추출에는 Node.js 또는 Deno도 필요할 수 있습니다. Node.js는 PATH 또는 `tools` 폴더에서, Deno는 yt-dlp의 기본 런타임 검색 경로에서 사용합니다. `yt-dlp.exe`와 `cloudflared.exe`는 `tools` 폴더·PATH에서 찾고, 없으면 처음 프록싱할 때 각 프로젝트의 공식 GitHub 릴리스에서 자동 다운로드합니다. 도구 갱신이 필요하면 앱을 닫고 `tools`의 해당 EXE를 새 버전으로 교체하세요. [yt-dlp 공식 안내](https://github.com/yt-dlp/yt-dlp#dependencies), [FFmpeg 다운로드](https://ffmpeg.org/download.html)

현재 최대 720p, 30분 이하, 파일당 1GB 미만의 녹화 영상을 지원하며 라이브는 제외합니다. 세션 캐시는 최대 4GB이고 앱 옆 `.relay-cache`에서 관리합니다. 정상 종료나 프록싱 해제 시 삭제하며, 비정상 종료로 남은 파일은 앱을 모두 닫은 뒤 이 폴더에서 지울 수 있습니다. 다운로드·변환 중에는 준비 중 상태를 표시하고 다른 검색이나 프록싱 해제로 취소할 수 있습니다.

중계 서버는 로컬 루프백 주소에만 열리고, 터널은 준비 완료된 MP4의 임의 토큰 경로만 공개합니다. GET·HEAD·단일 바이트 범위 요청을 지원하며 다운로드 제어 API나 PC 파일 목록은 공개하지 않습니다. 주소를 아는 사람은 영상을 재생할 수 있습니다.

VRChat에서 각 시청자는 `Allow Untrusted URLs`를 켜야 합니다. Public/Group Public 인스턴스는 월드 제작자가 해당 도메인을 허용해야 하므로, 매번 도메인이 바뀌는 임시 터널은 Friends/Invite 계열에서 테스트하는 편이 적합합니다. Android/Quest에는 HTTPS가 필요합니다. 실제 VRChat 플레이어 및 친구 측 네트워크 호환성은 별도 확인이 필요합니다. [VRChat 영상 URL 허용 규칙](https://creators.vrchat.com/worlds/udon/video-players/www-whitelist/)

자동 프록싱은 내 PC에서 작동하는 데스크톱 기능이며 Render 웹앱에서 실행되지 않습니다.

### 검증

```powershell
cargo test --features windows-app --lib --bins
# 실제 네트워크를 사용하는 선택 검사 (임시 터널 생성):
cargo test --features windows-app --lib relay::tests::live_tunnel_serves_a_generated_mp4 -- --ignored --nocapture
cargo test --features windows-app --lib relay::tests::live_youtube_download_and_hosting -- --ignored --nocapture
```

## 검색 특징

- `잇떼`, `DSCF`, `토키메키 포포롱`, `주문토끼 2기 OP` 같은 별칭·축약어를 보정합니다.
- 원곡·공식·원 가수 단서를 우선하고 커버·불러보았다·라이브 공연은 낮게 배치합니다.
- 노래방 모드에서는 KY/TJ/금영/태진/카라오케/반주 단서를 우선합니다.
- `OP`와 `ED`, 애니메이션 기수 정보를 구분합니다.
- 짧은 별칭을 원제와 연결합니다. 예: `두근어질 → きゅうくらりん`, `잇떼 → 言って。`, `DSCF → 妄想感傷代償連盟`.

## 구현

Rust 릴리스 최적화와 네이티브 Win32 유니코드 컨트롤로 제작했습니다. 한글 입력은 Windows IME가 직접 처리하며, UI 글꼴은 `Pretendard JP Variable`을 요청하고 미설치 환경에서는 Windows가 대체 글꼴을 사용합니다. `icon.jpg`를 가공한 다중 해상도 아이콘이 실행 파일에 내장됩니다.

Windows 데스크톱 앱을 소스에서 다시 빌드하려면 Rust 도구 모음 설치 후 이 폴더에서 `cargo build --release --features windows-app --bin music-link-finder`를 실행하면 됩니다.

## 웹앱 (한국어 · 모바일/데스크톱)

기존 Windows 프로그램은 그대로 보존합니다. 웹앱은 **동일한 `src/search.rs`** 로직을 서버에서 호출합니다. 별칭 확장, 점수화, 1순위/추천 분류 및 YouTube HTML 파싱은 변경하지 않았으며 **API 키나 비밀값이 필요 없습니다.** YouTube HTML 구조 또는 접근 정책 변경에 따라 검색이 실패할 수 있습니다.

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
- 결과 필드: `title`, `url`, `channel`, `score`, `matched`, `is_recommendation`. 정확 결과가 먼저, 추천 결과가 뒤에 옵니다.

```bash
cargo fmt --all -- --check
cargo test --bin music-link-finder-web --lib
cargo check --bin music-link-finder-web
```

웹에서는 개별 링크를 복사할 수 있으며, `첫 번째 링크 자동 복사` 스위치를 켜면 검색 완료 시 화면 맨 위 결과의 링크를 자동으로 복사합니다. 스위치는 페이지를 열 때 기본으로 꺼져 있습니다. 브라우저 보안 정책으로 복사가 차단되면 수동 복사 입력창을 보여 줍니다.

### Render 배포 (사용자 실행 단계)

1. 변경 사항을 GitHub 저장소에 푸시합니다.
2. Render 대시보드에서 **New → Blueprint**를 선택하고 저장소를 연결합니다.
3. 루트의 `render.yaml`을 인식하는지 확인한 후 **Apply**합니다. `runtime: docker`, Free 플랜, `/api/health` 헬스 체크가 설정됩니다.
4. 배포 후 Render가 발급한 `https://...onrender.com` 주소에서 검색을 확인합니다. **이 저장소 작업에서는 실제 배포하지 않습니다.**

수동 Web Service를 선택할 경우에도 저장소 루트의 `Dockerfile`을 이용해 **Docker** 런타임으로 배포하면 됩니다. 별도 Build/Start 명령, API 키 또는 환경변수 설정은 필요하지 않습니다. Render는 `PORT`를 주입하며 기본 포트는 `10000`입니다. Docker 시작 명령은 `/usr/local/bin/music-link-finder-web`입니다. 무료 인스턴스는 비활성 시 절전되어 첫 요청이 느릴 수 있습니다.
