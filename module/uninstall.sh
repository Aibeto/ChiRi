#!/system/bin/sh

# 先停掉守护进程：卸载后进程不会自动退出，可能继续锁频。
# killall 后循环壳因二进制被删会自行退出。
killall -9 yumi > /dev/null 2>&1
sleep 1

# 恢复被锁的 CPU 频率：强杀时 governor 可能卡在 performance 锁频，
# 放宽到硬件全档并退回 schedutil。
for d in /sys/devices/system/cpu/cpufreq/policy*; do
  [ -f "$d/scaling_available_frequencies" ] || continue
  max_f=$(tr ' ' '\n' < "$d/scaling_available_frequencies" | sort -n | tail -1)
  min_f=$(tr ' ' '\n' < "$d/scaling_available_frequencies" | sort -n | head -1)
  [ -n "$max_f" ] && echo "$max_f" > "$d/scaling_max_freq" 2>/dev/null
  [ -n "$min_f" ] && echo "$min_f" > "$d/scaling_min_freq" 2>/dev/null
  if grep -q schedutil "$d/scaling_available_governors" 2>/dev/null; then
    echo schedutil > "$d/scaling_governor" 2>/dev/null
  fi
done

# 恢复 OPPO/OnePlus/Realme 的 Oiface
# if [ -n "$(getprop persist.sys.oiface.enable)" ]; then
#   setprop persist.sys.oiface.enable 1
# fi

# 恢复小米的 Joyose 服务
# PACKAGE_NAME="com.xiaomi.joyose"
# if pm list packages | grep -q "$PACKAGE_NAME"; then
#   pm enable "$PACKAGE_NAME" >/dev/null 2>&1
# fi

echo "ChiRi 调度已卸载，重启手机生效"
