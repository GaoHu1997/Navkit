# -*- coding: utf-8 -*-
"""全部数据源 logo 统一取自 t8y2/dbx（apps/desktop/public/icons/database/）。

映射依据 dbx 的 DatabaseIcon.vue 资源表（含别名：cache->iris、argo->hive、
dameng->dm、inceptor->transwarp-inceptor.png 等）。dbx 缺失的 kind（druid / odbc）
保留既有素材。用法：python tools/fetch_dbx_logos.py
"""
import io
import os
import ssl
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "tools", "db_art")
BASE = "https://raw.githubusercontent.com/t8y2/dbx/main/apps/desktop/public/icons/database/%s"

ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE


def fetch(url, timeout=25):
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(req, timeout=timeout, context=ctx) as r:
            return r.read()
    except Exception:
        return None


# kind -> dbx 资源名（DatabaseIcon.vue 映射；无扩展名按 .svg/.png/.webp 探测）
MAP = {
    "mysql": "mysql", "postgresql": "postgres", "sqlserver": "sqlserver",
    "oracle": "oracle", "mariadb": "mariadb", "firebird": "firebird",
    "cockroachdb": "cockroachdb", "db2": "db2", "informix": "informix",
    "jdbcx": "jdbcx", "spanner": "spanner", "iris": "iris", "cache": "iris",
    "custom_mysql": "mysql", "dolt": "dolt", "custom_pg": "postgres",
    "cloudberry": "cloudberry", "clickhouse": "clickhouse", "databend": "databend",
    "databricks": "databricks", "saphana": "saphana", "teradata": "teradata",
    "vertica": "vertica.webp", "exasol": "exasol", "doris": "doris",
    "selectdb": "selectdb", "starrocks": "starrocks", "redshift": "redshift",
    "snowflake": "snowflake", "trino": "trino", "prestosql": "presto",
    "hive": "hive", "kyuubi": "kyuubi.png", "impala": "impala",
    "spark": "spark-logo.png", "dremio": "dremio", "bigquery": "bigquery",
    "kylin": "apache_kylin", "ignite": "apache_ignite", "salesforce": "salesforce",
    "opententbase": "opentenbase", "tidb": "tidb", "oceanbase": "oceanbase",
    "goldendb": "goldendb.png", "tdsql": "tdsql", "polardb": "polardb.webp",
    "greatsql": "greatsql", "gbase": "gbase.png", "opengauss": "opengauss",
    "gaussdb": "gaussdb", "kwdb": "kwdb", "kingbase": "kingbase",
    "highgo": "highgo.png", "uxdb": "uxdb", "yashandb": "yashandb.png",
    "vastbase": "vastbase", "dameng": "dm", "inceptor": "transwarp-inceptor.png",
    "argodb": "hive", "sundb": "sundb", "oscar": "oscar.png", "xugu": "xugu.png",
    "sqlite": "sqlite", "rqlite": "rqlite.png", "turso": "turso.png",
    "d1": "cloudflare-d1", "duckdb": "duckdb", "access": "access.png",
    "h2": "h2", "redis": "redis", "mongodb": "mongodb", "dynamodb": "dynamodb",
    "elasticsearch": "elasticsearch", "easysearch": "easysearch", "solr": "solr",
    "couchdb": "couchdb", "meilisearch": "meilisearch", "hbase": "hbase",
    "manticore": "manticoresearch.png", "cassandra": "cassandra",
    "qdrant": "qdrant", "milvus": "milvus.png", "weaviate": "weaviate",
    "chromadb": "chromadb", "neo4j": "neo4j", "nebulagraph": "nebula.png",
    "questdb": "questdb", "tdengine": "tdengine", "iotdb": "iotdb",
    "influxdb": "influxdb", "victoriametrics": "victoriametrics.png",
    "pulsar": "pulsar", "kafka": "kafka", "rocketmq": "rocketmq",
    "rabbitmq": "rabbitmq", "mqtt": "mqtt", "nacos": "nacos.png",
    "consul": "consul", "zookeeper": "zookeeper", "etcd": "etcd",
}

KEEP = ("off_druid.svg",)  # dbx 无此 kind，保留既有官方素材


def main():
    for f in os.listdir(OUT):
        if f.startswith("off_") and f not in KEEP:
            os.remove(os.path.join(OUT, f))
    ok, miss = [], []
    for kind, asset in MAP.items():
        names = [asset] if "." in asset else [asset + ".svg", asset + ".png", asset + ".webp"]
        got = False
        for nm in names:
            data = fetch(BASE % nm)
            if data and len(data) > 300:
                ext = nm.rsplit(".", 1)[1]
                io.open(os.path.join(OUT, "off_%s.%s" % (kind, ext)), "wb").write(data)
                print("OK  ", kind, "<-", nm, flush=True)
                got = True
                break
        if not got:
            miss.append(kind)
            print("MISS", kind, asset, flush=True)
    print("\n== dbx 素材：%d 成功，%d 缺失 ==" % (len(ok := [k for k in MAP if k not in miss]), len(miss)))
    if miss:
        print("缺失：", ", ".join(miss))


if __name__ == "__main__":
    main()
