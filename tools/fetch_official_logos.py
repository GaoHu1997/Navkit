# -*- coding: utf-8 -*-
"""采集各数据源的官方 logo 矢量素材 -> tools/db_art/off_<kind>.svg。

来源优先级：gilbarbara/logos（官方品牌矢量）> vectorlogo.zone > GitHub 仓库 README 资产。
只采官方 logo，不用任何自绘图形。用法：python tools/fetch_official_logos.py
"""
import io
import json
import os
import re
import ssl
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "tools", "db_art")

GB = "https://cdn.jsdelivr.net/gh/gilbarbara/logos@main/logos/%s.svg"
VLZ = "https://www.vectorlogo.zone/logos/%s/%s-ar21.svg"
RAW = "https://raw.githubusercontent.com/%s/%s/%s"

# kind -> 候选（依次尝试）：gb:<slug> | vlz:<slug> | gh:<owner/repo> | url:<直链>
CANDIDATES = {
    # ---- 关系型 ----
    "mysql": ["gb:mysql"], "postgresql": ["gb:postgresql"], "sqlite": ["gb:sqlite"],
    "sqlserver": ["gb:microsoft-sql-server", "gb:sql-server", "vlz:microsoftsqlserver"],
    "oracle": ["gb:oracle"], "mariadb": ["gb:mariadb"], "firebird": ["gb:firebird", "vlz:firebird"],
    "cockroachdb": ["gb:cockroachdb", "gh:cockroachdb/cockroach"],
    "db2": ["gb:db2", "gb:ibm-db2", "gh:ibmdb/go-db2"],
    "informix": ["gb:informix", "gb:ibm-informix"],
    "jdbcx": ["gb:java", "vlz:java"],  # JDBC 是 Java 家族 API，用 Java 官方标
    "spanner": ["gb:google-spanner", "gb:cloud-spanner", "gb:google-cloud"],
    "iris": ["gb:intersystems-iris", "gb:intersystems"],
    "cache": ["gb:intersystems-cache", "gb:intersystems"],
    "custom_mysql": ["gb:mysql"], "custom_pg": ["gb:postgresql"],
    "dolt": ["gb:dolt", "gh:dolthub/dolt"],
    "odbc": ["gb:odbc", "vlz:odbc"],
    # ---- 数仓分析 ----
    "cloudberry": ["gb:apache-cloudberry", "gh:apache/cloudberry"],
    "clickhouse": ["gb:clickhouse"], "databend": ["gb:databend", "gh:datafuselabs/databend"],
    "databricks": ["gb:databricks"], "saphana": ["gb:sap-hana", "gb:sap"],
    "teradata": ["gb:teradata"], "vertica": ["gb:vertica", "gh:vertica/vertica-python"],
    "exasol": ["gb:exasol"], "doris": ["gb:apache-doris", "gh:apache/doris"],
    "selectdb": ["gb:selectdb", "gh:apache/doris"],
    "starrocks": ["gb:starrocks", "gh:StarRocks/starrocks"],
    "redshift": ["gb:amazon-redshift", "gb:redshift"],
    "snowflake": ["gb:snowflake"], "trino": ["gb:trino"],
    "prestosql": ["gb:presto", "gb:prestodb"], "hive": ["gb:hive", "gb:apache-hive"],
    "kyuubi": ["gh:apache/kyuubi"], "impala": ["gb:impala", "gb:apache-impala"],
    "spark": ["gb:spark", "gb:apache-spark"], "dremio": ["gb:dremio"],
    "bigquery": ["gb:google-bigquery", "gb:bigquery"], "kylin": ["gb:kylin", "gb:apache-kylin"],
    "ignite": ["gb:ignite", "gb:apache-ignite"], "salesforce": ["gb:salesforce"],
    "druid": ["gb:druid", "gb:apache-druid"],
    # ---- 国产 ----
    "opententbase": ["gh:OpenTenBase/OpenTenBase"], "tidb": ["gb:tidb", "gh:pingcap/tidb"],
    "oceanbase": ["gb:oceanbase", "gh:oceanbase/oceanbase"],
    "goldendb": ["url:https://www.goldendb.com/images/logo.png"],
    "tdsql": ["url:https://cloud.tencent.com/product/tdsql"],
    "polardb": ["gb:polardb", "gh:polardb/PolarDB-X-Engine"],
    "greatsql": ["gh:greatsql/greatsql"], "gbase": ["url:https://www.gbase.cn/images/logo.png"],
    "opengauss": ["gh:opengauss-mirror/openGauss-server", "gb:opengauss"],
    "gaussdb": ["url:https://www.huaweicloud.com/product/gaussdb.html"],
    "kwdb": ["gh:kwdb/kwdb"], "kingbase": ["url:https://www.kingbase.com.cn/images/logo.png"],
    "highgo": ["url:https://www.highgo.com.cn/images/logo.png"],
    "uxdb": ["url:https://www.uxdb.com/images/logo.png"],
    "yashandb": ["gh:YashanDB/yashandb", "url:https://www.yashandb.com/images/logo.png"],
    "vastbase": ["gh:vastdata/Vastbase", "url:https://www.vastbase.com.cn/images/logo.png"],
    "inceptor": ["url:https://www.transwarp.cn/images/logo.png"], "argodb": ["url:https://www.transwarp.cn/images/logo.png"],
    "sundb": ["url:https://www.sundb.com.cn/images/logo.png"],
    "oscar": ["url:https://www.oscar.com.cn/images/logo.png"],
    "xugu": ["gh:Xugudb/xugu", "url:https://www.xugudb.com/images/logo.png"],
    "dameng": ["url:https://www.dameng.com/images/logo.png", "gh:eco-dameng/dmPython"],
    # ---- 轻量 ----
    "rqlite": ["gh:rqlite/rqlite"], "turso": ["gh:tursodatabase/turso-client-go", "gh:tursodatabase/libsql"],
    "d1": ["gb:cloudflare-d1", "gb:cloudflare-workers", "gh:cloudflare/d1-example"],
    "duckdb": ["gb:duckdb", "gh:duckdb/duckdb"],
    "access": ["gb:microsoft-access", "vlz:microsoftaccess"],
    "h2": ["gh:h2database/h2database", "url:https://h2database.com/html/images/h2-logo.png"],
    # ---- 文档/缓存/检索 ----
    "redis": ["gb:redis"], "mongodb": ["gb:mongodb"],
    "dynamodb": ["gb:amazon-dynamodb", "gb:dynamodb"],
    "elasticsearch": ["gb:elasticsearch"], "easysearch": ["gh:infinilabs/easysearch", "gh:infinilabs/analysis-ik"],
    "solr": ["gb:solr", "gb:apache-solr"], "couchdb": ["gb:couchdb"],
    "meilisearch": ["gb:meilisearch"], "hbase": ["gb:hbase", "gb:apache-hbase"],
    "manticore": ["gh:manticoresoftware/manticoresearch"],
    "cassandra": ["gb:cassandra", "gb:apache-cassandra"],
    # ---- 图谱/向量 ----
    "qdrant": ["gh:qdrant/qdrant"], "milvus": ["gb:milvus", "gh:milvus-io/milvus"],
    "weaviate": ["gh:weaviate/weaviate"], "chromadb": ["gh:chroma-core/chroma"],
    "neo4j": ["gb:neo4j"], "nebulagraph": ["gh:vesoft-inc/nebula"],
    # ---- 时序 ----
    "questdb": ["gh:questdb/questdb"], "tdengine": ["gh:taosdata/TDengine"],
    "iotdb": ["gh:apache/iotdb"], "influxdb": ["gb:influxdb"],
    "victoriametrics": ["gh:VictoriaMetrics/VictoriaMetrics"],
    # ---- 消息队列 ----
    "pulsar": ["gb:apache-pulsar", "gb:pulsar"], "kafka": ["gb:kafka", "gb:apache-kafka"],
    "rocketmq": ["gh:apache/rocketmq"], "rabbitmq": ["gb:rabbitmq"],
    "mqtt": ["gb:mqtt", "vlz:mqtt", "gh:mqtt/mqtt.org"],
    # ---- 注册与配置 ----
    "nacos": ["gh:alibaba/nacos"], "consul": ["gb:consul"],
    "zookeeper": ["gh:apache/zookeeper"], "etcd": ["gb:etcd", "gh:etcd-io/etcd"],
    # ---- 已有官方图形（也重采官方原版）----
    "mariadb2": [],
}

ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE


def fetch(url, timeout=15):
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    try:
        with urllib.request.urlopen(req, timeout=timeout, context=ctx) as r:
            return r.read()
    except Exception:
        return None


def is_svg(data):
    return data and b"<svg" in data[:2000] and len(data) > 300


def try_gh_logo(repo):
    """从 GitHub 仓库 README 里挖官方 logo 图（svg 优先）。"""
    for branch in ("master", "main", "develop"):
        readme = fetch(RAW % (repo, branch, "README.md"))
        if not readme:
            continue
        text = readme.decode("utf-8", "replace")
        urls = re.findall(r'!\[[^\]]*\]\(([^)]+)\)|<img[^>]+src="([^"]+)"', text)
        cands = []
        for a, b in urls:
            u = (a or b).strip()
            if re.search(r'logo|brand|icon', u, re.I) and re.search(r'\.svg\b', u, re.I):
                cands.append(u)
        for a, b in urls:
            u = (a or b).strip()
            if re.search(r'logo|brand|icon', u, re.I) and re.search(r'\.png\b', u, re.I):
                cands.append(u)
        for u in cands:
            if u.startswith("http"):
                data = fetch(u)
            elif u.startswith("/"):
                data = fetch(RAW % (repo, branch, u[1:]))
            else:
                data = fetch(RAW % (repo, branch, u))
            if is_svg(data):
                return data
    return None


def main():
    os.makedirs(OUT, exist_ok=True)
    ok, miss = [], []
    for kind, cands in CANDIDATES.items():
        if not cands:
            continue
        dest = os.path.join(OUT, "off_%s.svg" % kind)
        if os.path.exists(dest):
            ok.append(kind)
            continue
        got = None
        for c in cands:
            if c.startswith("gb:"):
                got = fetch(GB % c[3:])
            elif c.startswith("vlz:"):
                s = c[4:]
                got = fetch(VLZ % (s, s))
            elif c.startswith("gh:"):
                got = try_gh_logo(c[3:])
            elif c.startswith("url:"):
                got = fetch(c[4:])
            if is_svg(got):
                break
            got = None
        if got:
            io.open(dest, "wb").write(got)
            ok.append(kind)
            print("OK  ", kind, "<-", cands[0])
        else:
            miss.append(kind)
            print("MISS", kind)
    print("\n== 成功 %d，缺失 %d ==" % (len(ok), len(miss)))
    print("缺失：", ", ".join(miss))


if __name__ == "__main__":
    main()
