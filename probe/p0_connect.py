# P0 连通探针：只读，不动任何已有数据
import os
import json
import psycopg2

conn = psycopg2.connect(
    host=os.environ.get("GAUSSDB_HOST", "localhost"),
    port=int(os.environ.get("GAUSSDB_PORT", "5432")),
    user=os.environ.get("GAUSSDB_USER", "appuser"),
    password=os.environ.get("GAUSSDB_PASSWORD", ""),
    dbname=os.environ.get("GAUSSDB_DB", "n8n_test"),
    connect_timeout=10,
)
conn.autocommit = True
cur = conn.cursor()


def one(sql):
    try:
        cur.execute(sql)
        row = cur.fetchone()
        return row
    except Exception as e:  # 定档记录报错形态本身
        return "ERROR: {}".format(e).strip()


out = {}
out["version"] = one("SELECT version()")
out["server_version"] = one("SHOW server_version")
out["sql_compatibility"] = one("SHOW sql_compatibility")
out["enable_vectordb"] = one("SHOW enable_vectordb")
out["password_encryption_type"] = one("SHOW password_encryption_type")
out["databases"] = one(
    "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY 1"
)
out["current_user"] = one("SELECT current_user, current_database()")

print(json.dumps(out, ensure_ascii=False, indent=2, default=str))
cur.close()
conn.close()
