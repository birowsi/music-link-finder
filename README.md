# 음악 한글발음 링크 찾기

곡 제목을 입력하면 조건에 맞는 YouTube 영상을 비교하고, 가장 적합한 링크를 클립보드에 자동복사하는 Windows 전용 프로그램입니다.

## 실행

`MusicLinkFinder.exe` 또는 `run.bat`을 더블클릭하면 됩니다.

- Python 설치 불필요
- 별도 패키지 설치 불필요
- EXE 하나만 복사해도 단독 실행 가능
- YouTube API 키 불필요
- 다운로드 기능 없음

## 사용법

1. 곡 제목을 한글·영문·일문 중 편한 방식으로 입력합니다.
2. `한글발음 영상` 또는 `노래방 · 카라오케` 모드를 선택합니다.
3. Enter 또는 `검색 · 자동복사`를 누릅니다.
4. 1순위 링크가 자동으로 복사됩니다.

목록에서 다른 결과를 선택한 뒤 `선택 링크 복사`를 누르거나 결과를 더블클릭하면 해당 링크가 복사됩니다. `유튜브에서 열기` 버튼으로 선택한 영상을 브라우저에서 바로 열 수도 있습니다.

정확한 검색 결과 다음에는 같은 모드의 다른 추천곡이 이어집니다. 결과 영역 위에서 마우스 휠을 굴리면 계속 탐색할 수 있으며, 추천곡은 초록색 `추천` 표시로 정확 결과와 구분됩니다.

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

웹에서는 개별 링크를 복사할 수 있으며 브라우저 보안 정책으로 복사가 차단되면 수동 복사 입력창을 보여 줍니다. 검색이 끝났다고 링크를 자동 복사하지는 않습니다.

### Render 배포 (사용자 실행 단계)

1. 변경 사항을 GitHub 저장소에 푸시합니다.
2. Render 대시보드에서 **New → Blueprint**를 선택하고 저장소를 연결합니다.
3. 루트의 `render.yaml`을 인식하는지 확인한 후 **Apply**합니다. `runtime: docker`, Free 플랜, `/api/health` 헬스 체크가 설정됩니다.
4. 배포 후 Render가 발급한 `https://...onrender.com` 주소에서 검색을 확인합니다. **이 저장소 작업에서는 실제 배포하지 않습니다.**

수동 Web Service를 선택할 경우에도 저장소 루트의 `Dockerfile`을 이용해 **Docker** 런타임으로 배포하면 됩니다. 별도 Build/Start 명령, API 키 또는 환경변수 설정은 필요하지 않습니다. Render는 `PORT`를 주입하며 기본 포트는 `10000`입니다. Docker 시작 명령은 `/usr/local/bin/music-link-finder-web`입니다. 무료 인스턴스는 비활성 시 절전되어 첫 요청이 느릴 수 있습니다.
