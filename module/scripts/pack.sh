#!/system/bin/sh
# pack.sh —— 外部打包工具（tar 封装 + 可选 gzip）。守护进程与 WebUI 共用同一份，
# 属对外暴露的稳定接口：**本文件与 core/bin/tar（外部引入的 tar 二进制，可选）
# 不得被任何构建流程修改**（CI 的注释剥离已排除 scripts/ 目录，见 .github/workflows/build.yml）。
#
# 用法：
#   pack.sh archive <staging_dir> <out_tar>
#       把 staging_dir 打成 out_tar（无压缩；目标名由调用方定，便于唯一化）。
#       成功 exit 0；失败 exit 1（调用方负责保留 staging 并留痕）。
#   pack.sh export <src_dir> <dest_base> <fail_file> <progress_file> <total_file>
#       把 src_dir 打成 <dest_base>.tar.gz：先写 <dest_base>.tar，gzip 压缩后
#       **删除中间 .tar**（logd 归档里的 .tar 是正式产物，不在此列）。
#       gzip 不可用时保留未压缩 .tar 作为最终产物。
#       失败退出码写入 fail_file：3=源目录不可进，4=打包/压缩失败，5=源为空。
#       进度：tar -v 的行数计数写入 progress_file，总文件数预先写入 total_file。
#   exit 2 = 用法错误。
#
# tar 选择顺序：模块自带 core/bin/tar（外部二进制，优先）→ /system/bin/tar →
# toybox tar → busybox tar。
# gzip 选择顺序：gzip → busybox gzip；都没有则保留 .tar。

SELF_DIR=${0%/*}
MODDIR=${SELF_DIR%/*}

TAR_BIN=""
if [ -x "$MODDIR/core/bin/tar" ]; then
  TAR_BIN="$MODDIR/core/bin/tar"
elif [ -x /system/bin/tar ]; then
  TAR_BIN=/system/bin/tar
elif command -v toybox >/dev/null 2>&1; then
  TAR_BIN="toybox tar"
elif [ -n "$BUSYBOX" ] && "$BUSYBOX" tar --help >/dev/null 2>&1; then
  TAR_BIN="$BUSYBOX tar"
else
  TAR_BIN="tar"
fi

GZ_BIN=""
if command -v gzip >/dev/null 2>&1; then
  GZ_BIN="gzip"
elif [ -n "$BUSYBOX" ] && "$BUSYBOX" gzip --help >/dev/null 2>&1; then
  GZ_BIN="$BUSYBOX gzip"
fi

case "$1" in
  archive)
    D=$2
    T=$3
    [ -d "$D" ] || exit 1
    [ -n "$T" ] || exit 1
    [ -d "${T%/*}" ] || mkdir -p "${T%/*}" || exit 1
    # .part 写入 + 原子改名：中途失败不留半截 .tar
    $TAR_BIN -cf "$T.part" -C "$D" . || { rm -f "$T.part"; exit 1; }
    mv -f "$T.part" "$T" || { rm -f "$T.part"; exit 1; }
    exit 0
    ;;
  export)
    SRC=$2
    BASE=$3
    FAIL=$4
    PROG=$5
    TOTAL=$6
    [ -n "$SRC" ] && [ -n "$BASE" ] && [ -n "$FAIL" ] && [ -n "$PROG" ] || exit 2
    # 清理含上次异常中断的残留：.mode 必须一起删——否则本轮无 gzip 时会因旧
    # .mode 残留被判成「压缩中」（s:tar 不打印），前端轮询到超时
    rm -f "$BASE.tar.part" "$BASE.tar" "$BASE.tar.gz" "$FAIL" "$FAIL.mode" "$PROG"
    [ -d "$SRC" ] || { echo 3 > "$FAIL"; exit 3; }
    [ -n "$(ls -A "$SRC" 2>/dev/null)" ] || { echo 5 > "$FAIL"; exit 5; }
    if [ -n "$TOTAL" ]; then
      # tar -v 每处理一个条目打一行，**包含根目录条目 "./"**——总数必须同口径
      # （文件数 + 1），否则进度会显示 11/10、百分比永远差最后一格
      n=$(find "$SRC" -type f 2>/dev/null | wc -l)
      echo $((n + 1)) > "$TOTAL"
    fi
    : > "$PROG"
    $TAR_BIN -cvf "$BASE.tar.part" -C "$SRC" . >> "$PROG" 2>&1 || { rm -f "$BASE.tar.part"; echo 4 > "$FAIL"; exit 4; }
    mv -f "$BASE.tar.part" "$BASE.tar" || { rm -f "$BASE.tar.part"; echo 4 > "$FAIL"; exit 4; }
    if [ -n "$GZ_BIN" ]; then
      # 压缩中标记：调用方（WebUI）据此区分「gzip 进行中」与「无 gzip 的完成态」
      # （两者都会看到 .tar 存在）
      MODE="$FAIL.mode"
      echo gz > "$MODE"
      # gzip 就地生成 .tar.gz 并删除 .tar（默认即删源；-f 防覆盖交互提示）；
      # 再显式删一次兜底（不同实现行为差异）
      if $GZ_BIN -6 -f "$BASE.tar" 2>>"$PROG"; then
        rm -f "$BASE.tar" "$MODE"
      else
        rm -f "$MODE"
        echo 4 > "$FAIL"
        exit 4
      fi
    fi
    # 无 gzip：保留 .tar 作为最终产物（未压缩，用户可自行解包）；不写 MODE，
    # 完成态对调用方是「.tar 存在且无 MODE」
    exit 0
    ;;
  *)
    exit 2
    ;;
esac
