# P0 方言/类型/向量探针：使用独立库 seaorm_survey，不影响已有库
import json
import psycopg2
import psycopg2.extensions

BASE = dict(
    host="localhost",
    port=5432,
    user="appuser",
    password=json.load(open("probe/.creds.json"))["password"],
    connect_timeout=10,
)

results = {}


def run(cur, name, sql, params=None, fetch=True):
    try:
        cur.execute(sql, params)
        row = cur.fetchall() if fetch else None
        results[name] = row if fetch else "OK"
    except Exception as e:
        results[name] = "ERROR [{}]: {}".format(
            getattr(e, "pgcode", "?"), str(e).split("\n")[0]
        )


# --- 1. 建独立探针库（已存在则复用） ---
admin = psycopg2.connect(dbname="n8n_test", **BASE)
admin.autocommit = True
acur = admin.cursor()
acur.execute("SELECT 1 FROM pg_database WHERE datname = 'seaorm_survey'")
if acur.fetchone() is None:
    acur.execute("CREATE DATABASE seaorm_survey")
results["database_ready"] = "seaorm_survey"
acur.close()
admin.close()

# --- 2. 探针库内执行 ---
conn = psycopg2.connect(dbname="seaorm_survey", **BASE)
conn.autocommit = True
cur = conn.cursor()

# 类型探针
run(cur, "create_t_types",
    """CREATE TABLE t_types (id int PRIMARY KEY, c_number number(10,2),
       c_varchar2 varchar2(20), c_clob clob, c_blob blob, c_raw raw(16),
       c_date date, c_ts timestamp)""")
run(cur, "insert_t_types",
    "INSERT INTO t_types VALUES (1, 123.45, 'abc', 'clobtext', '00FF10', '0102ABCD', '2026-10-21', '2026-10-21 10:00:00')")
run(cur, "select_types_raw", "SELECT * FROM t_types")
run(cur, "catalog_types",
    """SELECT column_name, data_type, udt_name FROM information_schema.columns
       WHERE table_name = 't_types' ORDER BY ordinal_position""")
run(cur, "date_text_cast", "SELECT c_date::text, c_date::timestamp FROM t_types")
run(cur, "empty_string_is_null",
    """SELECT v IS NULL AS isnull FROM (INSERT INTO t_empty VALUES ('') RETURNING v) x"""
    if False else None)  # 占位，实际用下面单独表
run(cur, "create_t_empty", "CREATE TABLE t_empty (v varchar(10))")
run(cur, "insert_empty", "INSERT INTO t_empty VALUES ('')")
run(cur, "select_empty_isnull", "SELECT v IS NULL FROM t_empty")

# 方言探针
run(cur, "create_t_up", "CREATE TABLE t_up (id int PRIMARY KEY, v varchar(10))")
run(cur, "insert_up_1", "INSERT INTO t_up VALUES (1, 'a')")
run(cur, "on_conflict", "INSERT INTO t_up VALUES (1, 'b') ON CONFLICT (id) DO NOTHING")
run(cur, "on_duplicate_key", "INSERT INTO t_up VALUES (1, 'b') ON DUPLICATE KEY UPDATE v = excluded.v")
run(cur, "returning", "INSERT INTO t_up VALUES (2, 'c') RETURNING id")
run(cur, "with_recursive",
    """WITH RECURSIVE t(n) AS (VALUES (1) UNION ALL SELECT n+1 FROM t WHERE n < 5)
       SELECT sum(n) FROM t""")
run(cur, "lateral",
    "SELECT * FROM t_up LATERAL (SELECT t_up.v AS x) s")
run(cur, "to_regclass", "SELECT to_regclass('t_up')")
run(cur, "gen_random_uuid", "SELECT gen_random_uuid()")
run(cur, "create_if_not_exists", "CREATE TABLE IF NOT EXISTS t_if (x int)")
run(cur, "alter_add_col_if_not_exists", "ALTER TABLE t_if ADD COLUMN IF NOT EXISTS y int")
run(cur, "temp_table_serial", "CREATE TEMP TABLE tt (id serial)")
run(cur, "hash_index", "CREATE TABLE t_h (x int)", fetch=False)
run(cur, "hash_index2", "CREATE INDEX t_h_idx ON t_h USING hash (x)")

# 向量探针
run(cur, "pg_type_vector",
    "SELECT oid, typname, typcategory, typinput::text FROM pg_type WHERE typname LIKE '%vector%'")
run(cur, "create_vt",
    "CREATE TABLE vt (id int PRIMARY KEY, repr floatvector(3), b boolvector(3))")
run(cur, "insert_vt", "INSERT INTO vt VALUES (1, '[1.0,2.0,3.0]', '[1,0,1]')")
run(cur, "select_vt", "SELECT repr, b FROM vt")
run(cur, "l2_distance", "SELECT l2_distance(repr, '[4,5,6]') FROM vt")
run(cur, "cosine_distance", "SELECT cosine_distance(repr, '[4,5,6]') FROM vt")
run(cur, "arrow_op_order", "SELECT id FROM vt ORDER BY repr <-> '[0,0,0]' LIMIT 1")
run(cur, "arrow_op_param", "SELECT id FROM vt ORDER BY repr <-> %s LIMIT 1", ("[0,0,0]",))
run(cur, "insert_nan_element", "INSERT INTO vt VALUES (2, '[NaN,2,3]', '[1,0,1]')")
run(cur, "insert_wrong_dim", "INSERT INTO vt VALUES (3, '[1,2]', '[1,0,1]')")
run(cur, "inner_product", "SELECT inner_product('[1,2,3]', '[4,5,6]')")

# PL/SQL 复合语句经当前驱动（psycopg2 无参数走 simple query，预期成功留档）
run(cur, "plpgsql_function_simple",
    "CREATE OR REPLACE FUNCTION f_probe() RETURNS void AS $$ BEGIN NULL; END $$ LANGUAGE plpgsql")
run(cur, "call_function", "SELECT f_probe()")

print(json.dumps(results, ensure_ascii=False, indent=2, default=str))
cur.close()
conn.close()
