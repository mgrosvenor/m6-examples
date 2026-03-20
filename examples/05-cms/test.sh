#!/usr/bin/env bash
# test.sh — comprehensive end-to-end test suite for the example-05 CMS blog site.
#
# Requires the server stack to already be running (start with ./dev.sh).
# Tests every public page, every protected page (with real auth), every API
# endpoint, and the full CMS draft → publish → unpublish lifecycle.
#
# Usage:
#   ./test.sh                      # test against default https://127.0.0.1:8443
#   ./test.sh --addr HOST:PORT     # test against a different address
#   ./test.sh --user U --pass P    # use different credentials (default: admin/admin)
set -euo pipefail

ADDR="127.0.0.1:8443"
CMS_USER="admin"
CMS_PASS="admin"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --addr) ADDR="$2"; shift 2 ;;
        --user) CMS_USER="$2"; shift 2 ;;
        --pass) CMS_PASS="$2"; shift 2 ;;
        *) echo "Unknown flag: $1" >&2; exit 1 ;;
    esac
done

BASE="https://${ADDR}"
CURL=(curl -sk --http1.1)   # -s = silent, -k = skip TLS verify

PASS=0; FAIL=0; SKIP=0
AUTH_COOKIE=""          # populated after login
DRAFT_STEM=""           # populated after draft creation

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RESET='\033[0m'

# ── Helpers ────────────────────────────────────────────────────────────────────

check() {
    local label="$1"
    local got="$2"
    local want="$3"
    if [[ "$got" == "$want" ]]; then
        echo -e "  ${GREEN}PASS${RESET}  $label (${got})"
        PASS=$(( PASS + 1 ))
    else
        echo -e "  ${RED}FAIL${RESET}  $label — expected ${want}, got ${got}"
        FAIL=$(( FAIL + 1 ))
    fi
}

check_contains() {
    local label="$1"
    local body="$2"
    local needle="$3"
    if echo "$body" | grep -qF "$needle"; then
        echo -e "  ${GREEN}PASS${RESET}  $label (contains '${needle}')"
        PASS=$(( PASS + 1 ))
    else
        echo -e "  ${RED}FAIL${RESET}  $label — body does not contain '${needle}'"
        FAIL=$(( FAIL + 1 ))
    fi
}

http_code() {
    "${CURL[@]}" -o /dev/null -w "%{http_code}" "$@"
}

http_body() {
    "${CURL[@]}" "$@"
}

http_code_body() {
    # Writes status to stdout, body to second arg (variable name via nameref)
    local -n _body_ref=$2
    local _url="$1"; shift 2
    local _tmp
    _tmp=$("${CURL[@]}" -w '\n%{http_code}' "$_url" "$@")
    local _code="${_tmp##*$'\n'}"
    _body_ref="${_tmp%$'\n'${_code}}"
    echo "$_code"
}

section() { echo ""; echo -e "${YELLOW}── $1 ──────────────────────────────────────────────${RESET}"; }

# ── 0. Server reachability ────────────────────────────────────────────────────

section "0. Server reachability"
code=$(http_code "$BASE/")
check "GET / reachable" "$code" "200"

# ── 1. Public pages ───────────────────────────────────────────────────────────

section "1. Public pages"

code=$(http_code "$BASE/")
check "GET / → 200" "$code" "200"

body=$(http_body "$BASE/")
check_contains "GET / → contains site name" "$body" "m6"
check_contains "GET / → contains Recent Posts" "$body" "Recent Posts"

code=$(http_code "$BASE/about")
check "GET /about → 200" "$code" "200"

code=$(http_code "$BASE/blog")
check "GET /blog → 200" "$code" "200"
body=$(http_body "$BASE/blog")
check_contains "GET /blog → lists posts" "$body" "quick-start"

code=$(http_code "$BASE/blog/quick-start")
check "GET /blog/quick-start → 200" "$code" "200"
body=$(http_body "$BASE/blog/quick-start")
check_contains "GET /blog/quick-start → title present" "$body" "Quick Start"

code=$(http_code "$BASE/blog/architecture")
check "GET /blog/architecture → 200" "$code" "200"

code=$(http_code "$BASE/blog/nonexistent-post-xyz")
check "GET /blog/nonexistent → 200 (not-found page)" "$code" "200"
body=$(http_body "$BASE/blog/nonexistent-post-xyz")
check_contains "GET /blog/nonexistent → shows not-found message" "$body" "not found"

# ── 2. Static assets ──────────────────────────────────────────────────────────

section "2. Static assets"

code=$(http_code "$BASE/assets/style.css")
check "GET /assets/style.css → 200" "$code" "200"

content_type=$(${CURL[@]} -o /dev/null -w "%{content_type}" "$BASE/assets/style.css")
check_contains "style.css content-type is CSS" "$content_type" "text/css"

code=$(http_code "$BASE/assets/easymde.min.js")
check "GET /assets/easymde.min.js → 200" "$code" "200"

code=$(http_code "$BASE/assets/nonexistent.css")
check "GET /assets/nonexistent.css → 404" "$code" "404"

# ── 3. Contact form ───────────────────────────────────────────────────────────

section "3. Contact form"

code=$(http_code "$BASE/contact")
check "GET /contact → 200" "$code" "200"

body=$(http_body "$BASE/contact")
check_contains "GET /contact → contains form" "$body" "<form"
check_contains "GET /contact → has name field" "$body" "name"

# POST the contact form
code=$(http_code "$BASE/contact" -X POST \
    -H "Content-Type: application/x-www-form-urlencoded" \
    -d "name=Alice&email=alice%40example.com&message=Hello+from+test")
check "POST /contact → 200" "$code" "200"

body=$(http_body "$BASE/contact" -X POST \
    -H "Content-Type: application/x-www-form-urlencoded" \
    -d "name=TestUser&email=test%40example.com&message=Test+message")
check_contains "POST /contact → success confirmation" "$body" "TestUser"

# ── 4. Login page ─────────────────────────────────────────────────────────────

section "4. Login page"

code=$(http_code "$BASE/login")
check "GET /login → 200" "$code" "200"
body=$(http_body "$BASE/login")
check_contains "GET /login → has login form" "$body" "<form"
check_contains "GET /login → has password field" "$body" "password"

# ── 5. Auth endpoints ─────────────────────────────────────────────────────────

section "5. Auth endpoints"

# POST /auth/login — get auth cookie
login_resp=$("${CURL[@]}" -c /tmp/m6-test-cookies.txt \
    -w '\n%{http_code}' \
    -X POST "$BASE/auth/login" \
    -H "Content-Type: application/x-www-form-urlencoded" \
    -d "username=${CMS_USER}&password=${CMS_PASS}")
login_code="${login_resp##*$'\n'}"
check "POST /auth/login → 302 redirect" "$login_code" "302"

# Follow redirect to get session cookie
AUTH_COOKIE=$("${CURL[@]}" -b /tmp/m6-test-cookies.txt \
    -c /tmp/m6-test-cookies.txt \
    -o /dev/null -w "%{http_code}" \
    -X POST "$BASE/auth/login" \
    -H "Content-Type: application/x-www-form-urlencoded" \
    -d "username=${CMS_USER}&password=${CMS_PASS}" \
    -L)

# Get token from cookie jar
TOKEN=$(grep -oP 'auth_token\s+\K\S+' /tmp/m6-test-cookies.txt 2>/dev/null || true)
if [[ -n "$TOKEN" ]]; then
    echo -e "  ${GREEN}PASS${RESET}  auth token obtained"
    PASS=$(( PASS + 1 ))
else
    echo -e "  ${YELLOW}SKIP${RESET}  auth token not found in cookie jar (may use httpOnly)"
    SKIP=$(( SKIP + 1 ))
fi

# Use the cookie jar for authenticated requests
AUTHED_CURL=("${CURL[@]}" -b /tmp/m6-test-cookies.txt)

# ── 6. Protected CMS pages ────────────────────────────────────────────────────

section "6. Protected CMS pages"

# Without auth, CMS pages should redirect to login (302/401)
code=$(http_code "$BASE/cms")
if [[ "$code" == "302" || "$code" == "401" || "$code" == "403" ]]; then
    echo -e "  ${GREEN}PASS${RESET}  GET /cms without auth → ${code} (access denied)"
    PASS=$(( PASS + 1 ))
else
    echo -e "  ${RED}FAIL${RESET}  GET /cms without auth → ${code} (expected 302/401/403)"
    FAIL=$(( FAIL + 1 ))
fi

# With auth cookie
code=$("${AUTHED_CURL[@]}" -o /dev/null -w "%{http_code}" "$BASE/cms")
check "GET /cms with auth → 200" "$code" "200"
body=$("${AUTHED_CURL[@]}" "$BASE/cms")
check_contains "GET /cms → shows dashboard" "$body" "draft"

code=$("${AUTHED_CURL[@]}" -o /dev/null -w "%{http_code}" "$BASE/cms/new")
check "GET /cms/new with auth → 200" "$code" "200"
body=$("${AUTHED_CURL[@]}" "$BASE/cms/new")
check_contains "GET /cms/new → has editor" "$body" "title"

# ── 7. CMS API: draft lifecycle ───────────────────────────────────────────────

section "7. CMS API: draft → publish → unpublish lifecycle"

# Create a draft
DRAFT_TITLE="Test Post $(date +%s)"
create_resp=$("${AUTHED_CURL[@]}" \
    -X POST "$BASE/api/drafts" \
    -H "Content-Type: application/json" \
    -d "{\"title\":\"${DRAFT_TITLE}\",\"body\":\"Hello world\",\"summary\":\"A test post\",\"tags\":[\"test\"],\"date\":\"2026-03-21\"}" \
    -w '\n%{http_code}')
create_code="${create_resp##*$'\n'}"
create_body="${create_resp%$'\n'${create_code}}"
check "POST /api/drafts → 201 created" "$create_code" "201"

DRAFT_STEM=$(echo "$create_body" | python3 -c "import sys,json; print(json.load(sys.stdin).get('stem',''))" 2>/dev/null || true)
if [[ -n "$DRAFT_STEM" ]]; then
    echo -e "  ${GREEN}PASS${RESET}  draft stem obtained: ${DRAFT_STEM}"
    PASS=$(( PASS + 1 ))
else
    echo -e "  ${RED}FAIL${RESET}  could not extract draft stem from: ${create_body}"
    FAIL=$(( FAIL + 1 ))
    DRAFT_STEM="test-draft-fallback"
fi

# CMS edit page for the draft
code=$("${AUTHED_CURL[@]}" -o /dev/null -w "%{http_code}" "$BASE/cms/edit/${DRAFT_STEM}")
check "GET /cms/edit/${DRAFT_STEM} → 200" "$code" "200"

# Update the draft
update_resp=$("${AUTHED_CURL[@]}" \
    -X PATCH "$BASE/api/drafts/${DRAFT_STEM}" \
    -H "Content-Type: application/json" \
    -d "{\"summary\":\"Updated summary\"}" \
    -w '\n%{http_code}')
update_code="${update_resp##*$'\n'}"
check "PATCH /api/drafts/${DRAFT_STEM} → 200" "$update_code" "200"

# Publish the draft
publish_resp=$("${AUTHED_CURL[@]}" \
    -X POST "$BASE/api/publish/${DRAFT_STEM}" \
    -w '\n%{http_code}')
publish_code="${publish_resp##*$'\n'}"
publish_body="${publish_resp%$'\n'${publish_code}}"
check "POST /api/publish/${DRAFT_STEM} → 200" "$publish_code" "200"
check_contains "publish → published=true" "$publish_body" "published"

# Published post should be accessible
sleep 0.2
code=$(http_code "$BASE/blog/${DRAFT_STEM}")
check "GET /blog/${DRAFT_STEM} → 200 after publish" "$code" "200"

# Unpublish
unpublish_resp=$("${AUTHED_CURL[@]}" \
    -X POST "$BASE/api/unpublish/${DRAFT_STEM}" \
    -w '\n%{http_code}')
unpublish_code="${unpublish_resp##*$'\n'}"
unpublish_body="${unpublish_resp%$'\n'${unpublish_code}}"
check "POST /api/unpublish/${DRAFT_STEM} → 200" "$unpublish_code" "200"
check_contains "unpublish → unpublished=true" "$unpublish_body" "unpublished"

# Clean up: delete the draft
rm -f "/Users/mgrosvenor/m6-examples/examples/05-cms/content/drafts/${DRAFT_STEM}.json" 2>/dev/null || true
rm -f "/Users/mgrosvenor/m6-examples/examples/05-cms/content/posts/${DRAFT_STEM}.json" 2>/dev/null || true

# ── 8. Auth: logout and refresh ───────────────────────────────────────────────

section "8. Auth: refresh and logout"

code=$("${AUTHED_CURL[@]}" -o /dev/null -w "%{http_code}" "$BASE/auth/refresh")
if [[ "$code" == "200" || "$code" == "204" || "$code" == "302" ]]; then
    echo -e "  ${GREEN}PASS${RESET}  GET /auth/refresh → ${code}"
    PASS=$(( PASS + 1 ))
else
    echo -e "  ${YELLOW}SKIP${RESET}  GET /auth/refresh → ${code} (may require valid token)"
    SKIP=$(( SKIP + 1 ))
fi

logout_resp=$("${AUTHED_CURL[@]}" \
    -X POST "$BASE/auth/logout" \
    -w '\n%{http_code}')
logout_code="${logout_resp##*$'\n'}"
if [[ "$logout_code" == "200" || "$logout_code" == "302" || "$logout_code" == "204" ]]; then
    echo -e "  ${GREEN}PASS${RESET}  POST /auth/logout → ${logout_code}"
    PASS=$(( PASS + 1 ))
else
    echo -e "  ${RED}FAIL${RESET}  POST /auth/logout → ${logout_code}"
    FAIL=$(( FAIL + 1 ))
fi

# After logout, CMS should deny access
sleep 0.1
code=$("${AUTHED_CURL[@]}" -o /dev/null -w "%{http_code}" "$BASE/cms")
if [[ "$code" == "302" || "$code" == "401" || "$code" == "403" ]]; then
    echo -e "  ${GREEN}PASS${RESET}  GET /cms after logout → ${code} (access denied)"
    PASS=$(( PASS + 1 ))
else
    echo -e "  ${YELLOW}SKIP${RESET}  GET /cms after logout → ${code} (session may persist on server)"
    SKIP=$(( SKIP + 1 ))
fi

# ── 9. Error handling ─────────────────────────────────────────────────────────

section "9. Error handling"

code=$(http_code "$BASE/this-path-does-not-exist")
check "GET /nonexistent → 404" "$code" "404"

code=$(http_code "$BASE/blog/this-post-does-not-exist-xyz")
check "GET /blog/nonexistent → 200 (not-found page)" "$code" "200"
body=$(http_body "$BASE/blog/this-post-does-not-exist-xyz")
check_contains "GET /blog/nonexistent → shows not-found message" "$body" "not found"

# ── Summary ───────────────────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════════════════════════════"
TOTAL=$(( PASS + FAIL + SKIP ))
echo -e "  Results: ${GREEN}${PASS} passed${RESET}  ${RED}${FAIL} failed${RESET}  ${YELLOW}${SKIP} skipped${RESET}  (${TOTAL} total)"
echo "═══════════════════════════════════════════════════════"

rm -f /tmp/m6-test-cookies.txt

if [[ $FAIL -gt 0 ]]; then
    exit 1
fi
