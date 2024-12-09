#!/usr/bin/env python3

import argparse
import os
import pprint
import re
import shutil
import sys
from pathlib import Path

sys.path.append(os.path.realpath('ci/tests'))  # NOQA
import check_result

pp = pprint.PrettyPrinter()

NUM_DAYS = 10
TESTS = [
    "lxrust-benchs", "lxcpp-benchs", "lxtcutest",
    "chantests",
    "unittests", "hashmux-benchs", "hashmux-tests", "resmngtest",
    "rust-net-tests", "cpp-net-tests", "rust-net-benchs", "cpp-net-benchs",
    "rust-algo-tests", "rust-destr-tests", "rust-misc-tests", "rust-vfs-tests",
    "rust-algo-benchs", "rust-misc-benchs", "rust-vfs-benchs",
    "cpp-algo-benchs", "cpp-misc-benchs", "cpp-vfs-benchs",
    "facever", "rots-raser", "rots-hello",
    "find", "tar", "untar", "sqlite", "leveldb", "sha256sum", "sort",
    "cat_awk", "cat_wc", "grep_awk", "grep_wc",
    "disk-test", "abort-test",
    "standalone", "libctest", "rust-std-test", "msgchan", "rust-sndrcv", "vmtest",
    "ycsb-bench-udp", "ycsb-bench-tcp",
    "voiceassist-udp", "voiceassist-tcp",
    "bench-shell", "shell-nested", "parchksum", "filterchain"
]
COLORS = [
    'red', 'blue', 'green', 'orange', 'purple', 'yellow', 'black', 'lightgreen', 'lightblue'
]


def write_html_header(report):
    report.write("<!DOCTYPE html>\n")
    report.write("<html lang=\"en\">\n")
    report.write("<head>\n")
    report.write("  <title>M³ Unittests and Benchmarks</title>\n")
    report.write("  <meta charset=\"UTF-8\"/>\n")
    report.write("  <link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\">\n")
    report.write("</head>\n")
    report.write("<body>\n")


def write_html_footer(report):
    report.write("</body>\n")
    report.write("</html>\n")


def write_results(report, date, test):
    report.write("<h2>Results of {} on {}:</h2>".format(test, date))
    for cfg in results[date][test]:
        filename = "{}/tests/{}-{}-log.txt".format(date, test, cfg)
        report.write("<h3><a href=\"../{}\">{}</a></h3>".format(filename, cfg))
        res = results[date][test][cfg]
        report.write("<ul>\n")
        for failed in res.failures:
            report.write("  <li>{} <span class=\"failed\">failed</span></li>\n"
                         .format(failed))
        report.write("</ul>\n")


re_name = re.compile(
    r'^m3-tests-(' + '|'.join(TESTS) + ')-(a|b|sh|(?:hw|hw22|hw23)' +
    r'-(?:debug|bench)-(?:ex|sh))-(\S+?)-(\d+)$'
)

parser = argparse.ArgumentParser(description='Generates the website for the CI results.')
parser.add_argument('input')
parser.add_argument('output')
args = parser.parse_args()

if not os.path.exists(args.output):
    os.mkdir(args.output)

all_results = {}
for dir in Path(args.input).glob('*'):
    filename = os.path.basename(dir)
    if re.search(r'^\d+-\d+-\d+-[0-9a-f]+$', filename):
        all_results[filename] = dir

results = {}
# use only the last NUM_DAYS days
for key in sorted(all_results.keys())[-NUM_DAYS:]:
    try:
        dir = all_results[key]
        results[key] = {}
        for f in os.listdir(dir):
            match = re_name.match(f)
            if match:
                test = match.group(1)
                tiletype = match.group(2)
                isa = match.group(3)
                bpe = match.group(4)

                if test not in results[key]:
                    results[key][test] = {}
                subkey = "{}-{}-{}".format(tiletype, isa, bpe)
                logfile = str(dir) + '/' + str(f) + '/log.txt'
                logcopy = '{}/{}/tests/{}-{}-log.txt'.format(args.output, key, test, subkey)
                results[key][test][subkey] = check_result.parse_output(logfile)
                os.makedirs(os.path.dirname(logcopy), exist_ok=True)
                shutil.copyfile(logfile, logcopy)
            elif f == 'coverage':
                covdst = '{}/{}/coverage'.format(args.output, key)
                shutil.copytree(str(dir) + '/' + str(f), covdst)
    except Exception as e:
        print("warning: ignoring directory '{}': {}".format(key, e), file=sys.stderr)
        del results[key]

benchs = {}
cfgs = {}
for key in results:
    for test in results[key]:
        for cfg in results[key][test]:
            # only consider the benchmarks on gem5 with 64 blocks per extent
            if cfg[-3:] != "-64" or "hw-debug" in cfg or "hw22-debug" in cfg:
                continue
            res = results[key][test][cfg]
            for pname in res.perfs:
                benchs[pname] = 1
            cfgs[cfg] = 1

for key in results:
    outdir = args.output + '/' + key
    with open(outdir + '/log.html', 'w') as report:
        write_html_header(report)
        for test in results[key]:
            write_results(report, key, test)
        write_html_footer(report)

    for test in results[key]:
        with open(outdir + '/tests/' + test + '.html', 'w') as report:
            write_html_header(report)
            write_results(report, key, test)
            write_html_footer(report)

with open(args.output + '/style.css', 'w') as report:
    report.write("""
body {
    font-family: 'Helvetica';
    font-size: 12pt;
}
a {
    color: blue;
    text-decoration: none;
}
a:hover {
    text-decoration: underline;
}
table {
    border: solid 1px black;
    border-collapse: collapse;
    border-spacing: 1em;
    padding: 1em;
}
td, th {
    padding: 1em;
    border: solid 1px black;
}
th {
    background-color: #eeeeee;
}
.success {
    background-color: #cffdd2;
    color: white;
}
.failed {
    background-color: #fdcfcf;
    color: white;
}
span.failed {
    background-color: #fff;
    color: red;
}
""")

with open(args.output + '/index.html', 'w') as report:
    write_html_header(report)

    report.write("<script src=\"https://unpkg.com/chart.js@3\"></script>\n")

    report.write("<table>\n")
    report.write("  <tr>\n")
    report.write("  <th>Tests</th>\n")
    for key in sorted(results.keys()):
        report.write("    <th>\n")
        report.write("      {}<br/>\n".format(key[0:10]))
        report.write("      (<span title=\"{}\">{}</span>)<br/>\n"
                     .format(key[11:], key[11:19]))
        report.write("      <div style=\"margin-top: 0.2em\">\n")
        report.write("        <a href=\"{}/log.html\">Errors</a> &middot;\n".format(key))
        report.write("        <a href=\"{}/coverage/index.html\">Coverage</a>\n".format(key))
        report.write("      </div>\n")
        report.write("    </th>\n")
    report.write("  <th>Performance History</th>\n")
    report.write("  </tr>\n")

    for test in TESTS:
        report.write("  <tr>\n")
        report.write("    <td><a href=\"tests/{0}.html\">{0}</a></td>\n".format(test))
        for key in sorted(results.keys()):
            succ = 0
            fail = 0
            try:
                for cfg in results[key][test]:
                    res = results[key][test][cfg]
                    succ += res.succ_tests
                    fail += res.failed_tests
            except:
                pass
            report.write("    <td align=\"center\" class=\"{}\"><a href=\"{}/tests/{}.html\">{} / {}</a></td>\n"
                         .format("success" if fail == 0 else "failed", key, test, succ, fail + succ))

        # collect relative performance changes
        base = {}
        rel = {}
        for key in sorted(results.keys()):
            for cfg in cfgs:
                if cfg not in base:
                    base[cfg] = {}
                    rel[cfg] = {}

                try:
                    res = results[key][test][cfg]
                    for pname in res.perfs:
                        perf = res.perfs[pname]
                        # set all entries to invalid
                        if perf.name not in rel[cfg]:
                            rel[cfg][perf.name] = {}
                            for d in results:
                                rel[cfg][perf.name][d] = "null"

                        if perf.name in base[cfg]:
                            rel[cfg][perf.name][key] = str(perf.time / base[cfg][perf.name])
                        else:
                            base[cfg][perf.name] = perf.time
                            rel[cfg][perf.name][key] = "1"
                except Exception:
                    pass

        chart_name = 'changes_' + re.sub(r'[^a-zA-Z0-9_]', '', test)
        report.write("    <td>\n")
        report.write("    <div style=\"width: 300px; height: 80px\">\n")
        report.write("      <canvas id=\"{}\"></canvas>\n".format(chart_name))
        report.write("    </div>\n")
        report.write("    <script>\n")
        report.write("    var changeData{} = {{\n".format(chart_name))
        report.write("      labels: [")
        for key in sorted(results.keys()):
            report.write("\"{}\", ".format(key))
        report.write("      ],\n")
        report.write("      datasets: [\n")
        i = 0
        for cfg, rbenchs in rel.items():
            for name, vals in rbenchs.items():
                report.write("        {\n")
                report.write("          label: \"{}\",\n"
                             .format(cfg + " : " + name))
                report.write("          borderColor: \"{}\",\n"
                             .format(COLORS[i % len(COLORS)]))
                report.write("          pointRadius: 6,\n")
                report.write("          pointHoverRadius: 7,\n")
                report.write("          fill: false,\n")
                report.write("          lineTension: 0.1,\n")
                relvals = []
                for val_date in sorted(vals.keys()):
                    relvals.append(vals[val_date])
                report.write("          data: [{}],\n"
                             .format(', '.join(relvals)))
                report.write("        },\n")
                i += 1
        report.write("      ],\n")
        report.write("    }\n")
        report.write("    var {0} = document.getElementById(\"{0}\").getContext(\"2d\");\n"
                     .format(chart_name))
        report.write("    new Chart({}, {{\n".format(chart_name))
        report.write("      type: 'line',\n")
        report.write("      data: changeData{},\n".format(chart_name))
        report.write("      options: {\n")
        report.write("        responsive: true,\n")
        report.write("        plugins: { legend: { display: false } },\n")
        report.write("        scales: {\n")
        report.write("          x: { display: false },\n")
        report.write("          y: { suggestedMin: 0.9, suggestedMax: 1.1 },\n")
        report.write("        },\n")
        report.write("        maintainAspectRatio: false,\n")
        report.write("      },\n")
        report.write("    })\n")
        report.write("    </script>\n")
        report.write("    </td>\n")
        report.write("  </tr>\n")

    report.write("</table>\n")
    write_html_footer(report)

if not os.path.exists(args.output + '/tests'):
    os.mkdir(args.output + '/tests')

for test in TESTS:
    with open(args.output + '/tests/' + test + '.html', 'w') as report:
        write_html_header(report)

        report.write("<script src=\"https://unpkg.com/chart.js@3\"></script>\n")
        report.write("<script src=\"https://unpkg.com/chartjs-chart-error-bars@3\"></script>\n")
        report.write("<script>\n")
        report.write("Chart.defaults.font.family = 'Helvetica';\n")
        report.write("Chart.defaults.font.size = 16;\n")

        for bench in benchs:
            cfgdata = {}
            label = ''
            for cfg in cfgs:
                # collect the benchmark results
                tbenchs = {}
                for key in sorted(results.keys()):
                    if key in results and test in results[key] and cfg in results[key][test]:
                        res = results[key][test][cfg]
                        if bench in res.perfs:
                            perf = res.perfs[bench]
                            tbenchs[key] = (perf.time, perf.variance)
                            label = perf.unit
                # if none are part of this test, stop
                if len(tbenchs) == 0:
                    cfgdata[cfg] = []
                # otherwise put them into cfgdata and fill up missing results with 0
                else:
                    data = []
                    for key in sorted(results.keys()):
                        if key in tbenchs:
                            (time, var) = tbenchs[key]
                            data.append("{{ y: {}, yMin: {}, yMax: {} }}"
                                        .format(time, time - var, time + var))
                        else:
                            data.append("{ y: null, yMin: null, yMax: null }")
                    cfgdata[cfg] = data

            # skip benchmarks that are not part of this test
            if sum(len(cfgdata[item]) for item in cfgdata) == 0:
                continue

            chart_name = re.sub(r'[^a-zA-Z0-9_]', '', bench)
            report.write("var benchData{} = {{\n".format(chart_name))
            report.write("  labels: [")
            for key in sorted(results.keys()):
                report.write("\"{}\", ".format(key[0:19]))
            report.write("  ],\n")
            report.write("  datasets: [\n")
            i = 0
            for cfg in cfgs:
                report.write("    {\n")
                report.write("      label: \"{}\",\n".format(cfg))
                report.write("      borderColor: \"{}\",\n"
                             .format(COLORS[i % len(COLORS)]))
                report.write("      errorBarColor: \"{}\",\n"
                             .format(COLORS[i % len(COLORS)]))
                report.write("      errorBarWhiskerColor: \"{}\",\n"
                             .format(COLORS[i % len(COLORS)]))
                report.write("      pointRadius: 6,\n")
                report.write("      pointHoverRadius: 7,\n")
                report.write("      lineTension: 0.1,\n")
                report.write("      fill: false,\n")
                report.write("      data: [\n")
                for line in cfgdata[cfg]:
                    report.write("        {},\n".format(line))
                report.write("      ]\n")
                report.write("    },\n")
                i += 1
            report.write("  ],\n")
            report.write("}\n")
            report.write("</script>\n")

            report.write("<h2>{}</h2>\n".format(bench))
            report.write("<div style=\"width: 60%;\">\n")
            report.write("  <canvas id=\"chart_{}\"></canvas>\n"
                         .format(chart_name))
            report.write("</div>\n")
            report.write("<script>\n")
            report.write("var {0} = document.getElementById(\"chart_{0}\").getContext(\"2d\");\n"
                         .format(chart_name))
            report.write("new Chart({}, {{\n".format(chart_name))
            report.write("  type: 'lineWithErrorBars',\n")
            report.write("  data: benchData{},\n".format(chart_name))
            report.write("  options: {\n")
            report.write("    responsive: true,\n")
            report.write("    legend: {\n")
            report.write("      position: 'top',\n")
            report.write("    },\n")
            report.write("    scales: {\n")
            report.write("      y: {\n")
            report.write("        suggestedMin: 0,\n")
            report.write("        ticks: {\n")
            report.write("          callback: function(value, index, values) {")
            report.write(" return value + ' " + label + "'; }\n")
            report.write("        },\n")
            report.write("      },\n")
            report.write("    },\n")
            report.write("  },\n")
            report.write("})\n")

        report.write("</script>\n")
        write_html_footer(report)
