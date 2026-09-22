#!/usr/bin/env python3
"""devimp 日志包聚合分析（ChiRi 专用，详见 ../SKILL.md）。

用法:
    python analyze.py <解压目录> [--since MMDD-HHMMSS] [--min-n 30]

- 自动识别 48/44/40 列 schema（40 列旧 = 无尾部 8 列；48 列含 from_core；44 列含尾部 8 列无 from_core）
- 定版：读文件头 `# module=` / `# soc=` / `# android=`，混版本/混机型自动报警
- 充放电方向自动判定：分别统计 batt_i>0 / <0 两侧的功率中位数，取落在合理放电区间
  （0.05~30 W）的一侧；两测都合理或都异常时显式提示（该机型方向需人工确认）
- --since：只读文件名时间戳 >= 该值的 devimp 文件（新旧混装时筛最新一组）
"""
import argparse
import collections
import glob
import os
import re
import sys

# Windows 控制台默认 GBK：强制 UTF-8 输出并兜底替换，避免中文/符号打印崩掉
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

C48 = ("ts,type,mode,screen_on,pid,package,tid,comm,cluster,core,from_core,to_core,util_pct,max_util,"
       "over_cores,under_cores,cur_perf,tgt_perf,cur_freq_khz,max_freq_khz,decision,deb_up,deb_down,"
       "reason,pinned,thermal_cap_pct,touch,psi_cpu,psi_io,psi_mem,gpu_busy,batt_v,batt_i,batt_p,"
       "wakeups,migrations,freq_trans,batt_temp,cpu_temp,clg_active,cpu_cur_khz,cpu_max_khz,cpu_min_khz,"
       "cpu_governor,gpu_cur_khz,gpu_max_khz,gpu_min_khz,gpu_governor").split(",")
C44 = [c for c in C48 if c not in ("from_core", "to_core", "util_pct", "pinned")]
# 40 列旧版（Canary92 之前，如 8550e/Canary88）：= 48 列去掉尾部 8 列（cpu_cur_khz…gpu_governor）
C40 = C48[:40]


def num(v):
    try:
        return float(v)
    except Exception:
        return None


def avg(v):
    v = [x for x in v if x is not None]
    return sum(v) / len(v) if v else 0.0


def pct(v, q):
    v = sorted(x for x in v if x is not None)
    return v[min(int(len(v) * q), len(v) - 1)] if v else 0.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--since", default="0000-000000")
    ap.add_argument("--min-n", type=int, default=30)
    a = ap.parse_args()

    files = []
    pats = ("devimp_*.log", "main_*.log")  # aff_ 是文本帧流，不参与聚合
    for fn in sorted(sum((glob.glob(os.path.join(a.dir, p)) for p in pats), [])):
        m = re.search(r"_(\d{4}-\d{6})\.log$", os.path.basename(fn))
        if m and m.group(1) >= a.since:
            files.append(fn)
    if not files:
        print("没有符合条件的 devimp 文件"); sys.exit(1)

    # 定版第一件事：文件头 metadata（# module= / # soc= / # android=）。
    # 同名 tar 里可能混版本/混机型——多值即报警，跨包对比前必须一致。
    metas = collections.defaultdict(set)
    for fn in files[:8]:
        with open(fn, encoding="utf-8", errors="replace") as fh:
            for _ in range(6):  # 第 1 行是 CSV 表头，metadata 是紧随其后的 # 行
                ln = fh.readline()
                m = re.match(r"#\s*(module|soc|android)\s*=\s*(.+)", ln.strip())
                if m:
                    metas[m.group(1)].add(m.group(2))
    ver = " | ".join(f"{k}:{','.join(sorted(v))}" for k, v in metas.items())
    print(f"== 版本指纹 {ver or '(文件头无 metadata，旧版)'}"
          + ("   [!] 混版本/混机型" if any(len(v) > 1 for v in metas.values()) else ""))

    # schema：首行表头判定（44 列版末 8 列为 cpu_cur_khz…；48 列含 from_core）
    header = open(files[0], encoding="utf-8", errors="replace").readline()
    if "cpu_cur_khz" not in header:
        cols, tag = C40, "40列旧(Canary92前)"
    elif "from_core" in header:
        cols, tag = C48, "48列(Canary92期)"
    else:
        cols, tag = C44, "44列新(拆分版)"
    I = {n: i for i, n in enumerate(cols)}
    ncol = len(cols)

    rows = []
    types = collections.Counter()
    last_ts = ""
    for fn in files:
        for ln in open(fn, encoding="utf-8", errors="replace"):
            if not ln.strip() or ln.startswith(("ts,", "#")):
                continue
            p = ln.rstrip("\n").split(",")
            if len(p) < 40:
                continue
            types[p[I["type"]]] += 1
            last_ts = max(last_ts, p[0])
            rows.append(p)

    # 充放电方向判定：两侧功率中位数
    pos = [num(p[I["batt_p"]]) for p in rows if p[I["type"]] == "snap" and (num(p[I["batt_i"]]) or 0) > 0]
    neg = [num(p[I["batt_p"]]) for p in rows if p[I["type"]] == "snap" and (num(p[I["batt_i"]]) or 0) < 0]
    pos = [v for v in pos if v is not None]
    neg = [v for v in neg if v is not None]
    def sane(v):
        return v and 0.05 <= pct(v, 0.5) <= 30.0
    if sane(pos) and not sane(neg):
        disch_positive, note = True, "（枚举：batt_i>0 侧为放电）"
    elif sane(neg) and not sane(pos):
        disch_positive, note = False, "（枚举：batt_i<0 侧为放电）"
    else:
        disch_positive, note = True, ("[!] 两侧功率都『合理』或都异常，方向需人工确认；"
                                      "本次按 batt_i>0 侧统计")
        if not sane(pos) and not sane(neg):
            note = "[!] 两侧样本都很小或功率异常（可能刚重启/短包），请人工核对"

    def is_discharge(p):
        v = num(p[I["batt_i"]])
        return v is not None and (v > 0) == disch_positive

    agg = collections.defaultdict(lambda: collections.defaultdict(list))
    tick = collections.defaultdict(lambda: collections.defaultdict(list))
    top = collections.defaultdict(lambda: collections.defaultdict(float))
    topn = collections.defaultdict(int)
    for p in rows:
        t = p[I["type"]]
        key = (p[I["mode"]], p[I["package"]] or "nopkg")
        if t == "snap":
            if not is_discharge(p):
                continue
            d = agg[key]
            d["n"].append(1)
            d["P"].append(num(p[I["batt_p"]]))
            d["bt"].append(num(p[I["batt_temp"]]))
            d["ct"].append(num(p[I["cpu_temp"]]))
            d["cap"].append(num(p[I["thermal_cap_pct"]]))
            d["gpu"].append(num(p[I["gpu_busy"]]))
            d["psi"].append(num(p[I["psi_cpu"]]))
            d["mig"].append(num(p[I["migrations"]]))
        elif t == "tick":
            cl = p[I["cluster"]]
            if cl not in ("little", "big", "prime"):
                continue
            c, mx = num(p[I["cur_freq_khz"]]), num(p[I["max_freq_khz"]])
            if c and mx:
                tick[key][cl + "_cap"].append(c / mx * 100)
            tick[key][cl + "_u"].append(num(p[I["max_util"]]))
        elif t == "tgtop":
            # 48 列：7=comm、12=占用率；44 列：comm 仍为 7、占用率列见 SKILL.md（tgtop 字段位置
            # 不随 schema 名称表变化，两版一致取 7 / 12）
            top[key][p[7] if len(p) > 7 else "?"] += num(p[12]) or 0.0
            topn[key] += 1

    print(f"== {a.dir}  files={len(files)}  schema={tag}  last_ts={last_ts}")
    print(f"   放电方向: {'batt_i>0' if disch_positive else 'batt_i<0'} 侧 {note}")
    print(f"   行类型: {dict(types)}")
    print(f"{'mode':9s} {'package':26s} {'n':>5s} {'P_avg':>6s} {'p50':>5s} {'p95':>6s} {'battT':>5s} "
          f"{'cpuT':>5s} {'cap':>4s} {'gpu':>5s} {'psi':>5s} {'mig':>6s} | "
          f"{'little':>15s} {'big':>15s} {'prime':>15s}")
    for k in sorted(agg, key=lambda k: -len(agg[k]["P"])):
        d = agg[k]
        if len(d["P"]) < a.min_n:
            continue
        tk = tick.get(k, {})
        def cc(cl):
            return (f"{avg(tk.get(cl+'_cap', [])):5.1f}/{pct(tk.get(cl+'_cap', []), .95):5.1f} "
                    f"u{avg(tk.get(cl+'_u', [])):.2f}")
        print(f"{k[0]:9s} {k[1][:26]:26s} {len(d['P']):5d} {avg(d['P']):6.2f} {pct(d['P'], .5):5.2f} "
              f"{pct(d['P'], .95):6.2f} {avg(d['bt']):5.1f} {avg(d['ct']):5.1f} {avg(d['cap']):4.0f} "
              f"{avg(d['gpu']):5.1f} {avg(d['psi']):5.1f} {avg(d['mig']):6.0f} | {cc('little')} "
              f"{cc('big')} {cc('prime')}")
    print("--- tgtop 线程占比（近全时段） ---")
    for k in sorted(top, key=lambda k: -topn[k])[:8]:
        tot = sum(top[k].values()) or 1.0
        items = sorted(top[k].items(), key=lambda x: -x[1])[:6]
        print(f"  {k[0]:9s} {k[1][:24]:24s} n={topn[k]:4d} " +
              " ".join(f"{n}:{v/tot*100:.1f}%" for n, v in items))


if __name__ == "__main__":
    main()
