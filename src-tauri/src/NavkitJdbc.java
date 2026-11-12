import java.sql.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

// Java 11 source launcher. Credentials travel only on stdin, never argv.
class NavkitJdbc {
  static Connection conn;
  static JdbcMutationSupport relational;
  static String quote = "\"", password = "", driverClass = "";
  static String initialCatalog, initialSchema;
  static final String BIGQUERY_DRIVER = "com.google.cloud.bigquery.jdbc.BigQueryDriver";
  static final String SPANNER_DRIVER = "com.google.cloud.spanner.jdbc.JdbcDriver";
  static final String ARGODB_DRIVER = "io.transwarp.jdbc.QuarkDriver";
  // Preserve the IPC writer created by main, but do not let this driver's internal URL
  // (which includes credential Properties) escape through stdout/stderr or JUL handlers.
  static void suppressCredentialDriverLogs() throws IOException {
    System.setOut(new PrintStream(OutputStream.nullOutputStream(), true, StandardCharsets.UTF_8.name()));
    System.setErr(new PrintStream(OutputStream.nullOutputStream(), true, StandardCharsets.UTF_8.name()));
    java.util.logging.LogManager.getLogManager().reset();
    java.util.logging.Logger.getLogger("").setLevel(java.util.logging.Level.OFF);
  }
  static boolean qualifiedCatalogs, catalogOnly;
  static final Map<String, String[]> namespaces = new LinkedHashMap<>();
  static final long MAX_OFFSET = 1000000, MAX_PAGE = 10000;

  static String decode(String s) { return new String(Base64.getDecoder().decode(s), StandardCharsets.UTF_8); }
  static String json(String s) {
    if (s == null) return "null";
    StringBuilder b = new StringBuilder("\"");
    for (char c : s.toCharArray()) {
      switch (c) {
        case '"': b.append("\\\""); break;
        case '\\': b.append("\\\\"); break;
        case '\n': b.append("\\n"); break;
        case '\r': b.append("\\r"); break;
        case '\t': b.append("\\t"); break;
        default: if (c < 32) b.append(String.format("\\u%04x", (int)c)); else b.append(c);
      }
    }
    return b.append('"').toString();
  }
  static String q(String ident) throws SQLException {
    if (ident == null || ident.isEmpty() || ident.indexOf(0) >= 0) throw new SQLException("Invalid JDBC identifier", "42000");
    if (quote == null || quote.isEmpty()) {
      if (!ident.matches("[A-Za-z_][A-Za-z0-9_]*")) throw new SQLException("Driver has no safe identifier quoting", "42000");
      return ident;
    }
    return quote + ident.replace(quote, quote + quote) + quote;
  }
  static void error(PrintWriter out, Exception e) {
    String state = e instanceof SQLException ? ((SQLException)e).getSQLState() : "";
    String detail = e.getClass().getSimpleName() + " [" + state + "]: " + e.getMessage();
    if (driverClass.equals(BIGQUERY_DRIVER) && detail.contains("OAuth2Credentials instance does not support refreshing the access token")) {
      detail = "BigQuery OAuth authentication [28000]: the pre-generated token is expired or rejected and cannot be refreshed; replace the token and reconnect. " + detail;
    }
    if (!password.isEmpty()) detail = detail.replace(password, "[REDACTED]");
    out.println("{\"error\":" + json(detail) + "}");
  }
  static String cell(ResultSet rs, ResultSetMetaData metadata, int index) throws SQLException {
    switch (metadata.getColumnType(index)) {
      case Types.BINARY: case Types.VARBINARY: case Types.LONGVARBINARY: case Types.BLOB:
        byte[] bytes = rs.getBytes(index);
        return bytes == null ? null : "base64:" + Base64.getEncoder().encodeToString(bytes);
      default: return rs.getString(index);
    }
  }
  static void emit(PrintWriter out, ResultSet rs, long offset, long limit) throws SQLException {
    ResultSetMetaData m = rs.getMetaData();
    int n = m.getColumnCount();
    StringBuilder b = new StringBuilder("{\"columns\":[");
    for (int i=1; i<=n; i++) { if (i>1) b.append(','); b.append(json(m.getColumnLabel(i))); }
    b.append("],\"rows\":[");
    long skipped=0, produced=0;
    while (rs.next()) {
      if (skipped < offset) { skipped++; continue; }
      if (limit >= 0 && produced >= limit) break;
      if (++produced > MAX_PAGE) throw new SQLException("Query exceeds 10000 rows; use explicit LIMIT/paging. No partial result returned.", "54000");
      if (produced>1) b.append(',');
      b.append('[');
      for (int i=1; i<=n; i++) { if(i>1)b.append(','); b.append(json(cell(rs,m,i))); }
      b.append(']');
    }
    out.println(b.append("]}").toString());
  }
  static void rowsJson(PrintWriter out, List<String[]> rows, String[] cols) {
    StringBuilder b = new StringBuilder("{\"columns\":[");
    for (int i=0; i<cols.length; i++) { if(i>0)b.append(','); b.append(json(cols[i])); }
    b.append("],\"rows\":[");
    boolean first=true;
    for (String[] row:rows) {
      if(!first)b.append(','); first=false; b.append('[');
      for(int i=0;i<row.length;i++) { if(i>0)b.append(','); b.append(json(row[i])); }
      b.append(']');
    }
    out.println(b.append("]}").toString());
  }
  static String catalog() throws SQLException {
    try { return conn.getCatalog(); } catch(SQLFeatureNotSupportedException e) { return null; }
  }
  static String schema() throws SQLException {
    try { return conn.getSchema(); } catch(SQLFeatureNotSupportedException e) { return null; }
  }
  static boolean system(String s) {
    return s==null || s.isEmpty() || Arrays.asList("pg_catalog","information_schema","sys","mysql","performance_schema","syscat","sysstat","sysibm","syssegments","innodb").contains(s.toLowerCase(Locale.ROOT));
  }
  static void discoverNamespaces() throws SQLException {
    DatabaseMetaData md=conn.getMetaData();
    LinkedHashMap<String,String[]> found=new LinkedHashMap<>();
    if(!catalogOnly) {
      try(ResultSet rs=md.getSchemas()) {
        while(rs.next()) {
          String name=rs.getString("TABLE_SCHEM"); if(system(name))continue;
          if(qualifiedCatalogs) {
            String cat=rs.getString("TABLE_CATALOG");
            if(cat==null || cat.isEmpty()) throw new SQLException("JDBC schema metadata lacks catalog identity", "3F000");
            found.put(cat+"/"+name,new String[]{cat,name});
          } else found.put(name,new String[]{initialCatalog,name});
        }
      } catch(SQLFeatureNotSupportedException unsupported) { if(qualifiedCatalogs)throw unsupported; }
    }
    if(found.isEmpty() && !qualifiedCatalogs) {
      try(ResultSet rs=md.getCatalogs()) {
        while(rs.next()) { String cat=rs.getString("TABLE_CAT"); if(!system(cat))found.put(cat,new String[]{cat,null}); }
      } catch(SQLFeatureNotSupportedException unsupported) {
        // UCanAccess exposes table metadata and the actual current catalog, but
        // explicitly does not implement catalog/schema enumeration. Preserve that
        // real file namespace; never invent a default for another JDBC driver.
        if(!driverClass.equals("net.ucanaccess.jdbc.UcanaccessDriver"))throw unsupported;
        String current=catalog(),currentSchema=schema();
        String name=current!=null && !current.isEmpty()?current:currentSchema;
        if(name==null || name.isEmpty())throw unsupported;
        found.put(name,new String[]{current,currentSchema});
      }
    }
    namespaces.clear(); namespaces.putAll(found);
  }
  static String[] namespace(String value) throws SQLException {
    if(value==null || value.isEmpty())return new String[]{initialCatalog,initialSchema};
    if(value.indexOf(0)>=0)throw new SQLException("Namespace contains NUL", "3F000");
    String[] ns=namespaces.get(value);
    if(ns==null) { discoverNamespaces(); ns=namespaces.get(value); }
    if(ns==null)throw new SQLException("JDBC namespace not found or inaccessible: "+value, "3F000");
    return ns;
  }
  // Every command selects/restores namespace while the Rust sidecar mutex is held.
  static void selectNamespace(String value) throws SQLException {
    String[] ns=namespace(value);
    if(!Objects.equals(catalog(),ns[0])) {
      conn.setCatalog(ns[0]);
      if(!Objects.equals(catalog(),ns[0]))throw new SQLException("Driver did not select the requested catalog", "3D000");
    }
    if(!Objects.equals(schema(),ns[1])) {
      conn.setSchema(ns[1]);
      if(!Objects.equals(schema(),ns[1]))throw new SQLException("Driver did not select the requested schema", "3F000");
    }
  }
  static void query(PrintWriter out,String sql,int timeout,String container) throws SQLException {
    selectNamespace(container);
    try(Statement st=conn.createStatement()) {
      st.setQueryTimeout(timeout); st.setMaxRows(10001);
      boolean has=st.execute(sql);
      if(!has) {
        long affected=0;
        if(!conn.isClosed()) {
          try { affected=st.getLargeUpdateCount(); }
          catch(SQLFeatureNotSupportedException | UnsupportedOperationException | AbstractMethodError unsupported) { affected=st.getUpdateCount(); }
        }
        rowsJson(out,Collections.singletonList(new String[]{Long.toString(Math.max(0,affected))}),new String[]{"affected"});
      } else try(ResultSet rs=st.getResultSet()) { emit(out,rs,0,-1); }
    }
  }
  static void ping(PrintWriter out,int timeout) throws SQLException {
    // Cloud JDBC connects can be lazy: require a real query before reporting a connection.
    boolean fallback=driverClass.equals(BIGQUERY_DRIVER) || driverClass.equals(SPANNER_DRIVER);
    if(!fallback) {
      try { if(!conn.isValid(timeout))throw new SQLException("JDBC connection is not valid", "08003"); }
      catch(SQLFeatureNotSupportedException | AbstractMethodError unsupported) { fallback=true; }
    }
    if(fallback) {
      String sql=driverClass.equals("oracle.jdbc.OracleDriver")?"SELECT 1 FROM DUAL":
        driverClass.equals("org.firebirdsql.jdbc.FBDriver")?"SELECT 1 FROM RDB$DATABASE":
        driverClass.equals("com.ibm.db2.jcc.DB2Driver")?"VALUES 1":
        driverClass.equals("com.informix.jdbc.IfxDriver")?"SELECT FIRST 1 1 FROM systables":"SELECT 1";
      try(Statement st=conn.createStatement()) { st.setQueryTimeout(timeout); try(ResultSet rs=st.executeQuery(sql)) { if(!rs.next())throw new SQLException("JDBC validation query returned no row", "08003"); } }
    }
    rowsJson(out,Collections.singletonList(new String[]{"valid"}),new String[]{"connection"});
  }
  static void containers(PrintWriter out) throws SQLException {
    selectNamespace(""); discoverNamespaces();
    List<String[]> rows=new ArrayList<>();
    for(Map.Entry<String,String[]> entry:namespaces.entrySet()) rows.add(new String[]{entry.getKey(),qualifiedCatalogs?"catalog/schema":entry.getValue()[1]==null || entry.getValue()[1].isEmpty()?"catalog":"schema"});
    rowsJson(out,rows,new String[]{"name","kind"});
  }
  static void tables(PrintWriter out,String container) throws SQLException {
    selectNamespace(container); String[] ns=namespace(container);
    List<String[]> rows=new ArrayList<>();
    try(ResultSet rs=conn.getMetaData().getTables(ns[0],ns[1],"%",new String[]{"TABLE","VIEW","BASE TABLE"})) {
      while(rs.next()) { String name=rs.getString("TABLE_NAME"); if(name!=null)rows.add(new String[]{name,"VIEW".equalsIgnoreCase(rs.getString("TABLE_TYPE"))?"view":"table"}); }
    }
    rowsJson(out,rows,new String[]{"name","type"});
  }
  static void rows(PrintWriter out,String container,String table,long offset,long limit,int timeout) throws SQLException {
    if(offset<0 || offset>MAX_OFFSET || limit<0 || limit>MAX_PAGE || offset+limit>MAX_OFFSET)
      throw new SQLException("JDBC preview exceeds explicit paging safety bounds (offset+limit <= 1000000, limit <= 10000)", "54000");
    selectNamespace(container);
    try(Statement st=conn.createStatement()) {
      st.setQueryTimeout(timeout); st.setMaxRows((int)Math.max(1,offset+limit));
      try(ResultSet rs=st.executeQuery("SELECT * FROM "+q(table))) { emit(out,rs,offset,limit); }
    }
  }
  static void columns(PrintWriter out,String container,String table) throws SQLException {
    selectNamespace(container); String[] ns=namespace(container); DatabaseMetaData md=conn.getMetaData();
    Set<String> pks=new HashSet<>();
    try(ResultSet rs=md.getPrimaryKeys(ns[0],ns[1],table)) { while(rs.next())pks.add(rs.getString("COLUMN_NAME")); }
    catch(SQLFeatureNotSupportedException unsupported) { }
    List<String[]> rows=new ArrayList<>();
    try(ResultSet rs=md.getColumns(ns[0],ns[1],table,"%")) {
      while(rs.next()) { String name=rs.getString("COLUMN_NAME"); rows.add(new String[]{name,rs.getString("TYPE_NAME"),rs.getInt("NULLABLE")!=DatabaseMetaData.columnNoNulls?"true":"false",pks.contains(name)?"true":"false",Integer.toString(rs.getInt("COLUMN_SIZE")),Integer.toString(rs.getInt("DECIMAL_DIGITS"))}); }
    }
    rowsJson(out,rows,new String[]{"name","type","nullable","pk","size","scale"});
  }
  // Offline readiness validates loading/SPI/URL parsing. It is not a server test.
  static void probe(String driverClass,String url) throws Exception {
    if(javax.tools.ToolProvider.getSystemJavaCompiler()==null)throw new IllegalStateException("jdk.compiler/source launcher is required");
    if(driverClass.equals("com.informix.jdbc.IfxDriver"))Class.forName("org.bson.BsonDocument");
    if(driverClass.equals("org.apache.kylin.jdbc.Driver")) {
      Class.forName("javax.xml.bind.DatatypeConverter"); Class.forName("javax.activation.DataSource");
    }
    if(driverClass.equals("com.xugu.cloudjdbc.Driver")) {
      Class.forName("org.checkerframework.checker.nullness.NullnessUtil");
      Class<?> geometry=Class.forName("com.xugu.geometric.DBbox");
      Object original=geometry.getDeclaredConstructor(String.class).newInstance("(1,2),(3,4)");
      byte[] bytes=new byte[(Integer)geometry.getMethod("lengthInBytes").invoke(original)];
      geometry.getMethod("toBytes",byte[].class,int.class).invoke(original,bytes,0);
      Object decoded=geometry.getDeclaredConstructor().newInstance();
      geometry.getMethod("setByteValue",byte[].class,int.class).invoke(decoded,bytes,0);
      if(!original.equals(decoded))throw new IllegalStateException("Xugu geometry runtime dependency round trip failed");
    }
    Object instance=Class.forName(driverClass).getDeclaredConstructor().newInstance();
    if(!(instance instanceof Driver))throw new IllegalArgumentException("Class is not java.sql.Driver");
    Driver driver=(Driver)instance;
    if(!driver.acceptsURL(url))throw new SQLException("Driver rejects the product JDBC URL");
    boolean spi=false;
    for(Driver candidate:ServiceLoader.load(Driver.class)) {
      String registered=candidate.getClass().getName();
      // The official ArgoDB Quark JAR registers its inherited HiveDriver SPI;
      // QuarkDriver itself remains the exact product class used below.
      if(registered.equals(driverClass) || (driverClass.equals(ARGODB_DRIVER) && registered.equals("io.transwarp.hive.jdbc.HiveDriver")))spi=true;
    }
    if(!spi)throw new IllegalStateException("JDBC SPI does not expose the requested product driver");
    String selected=DriverManager.getDriver(url).getClass().getName();
    if(!selected.equals(driverClass) && !(driverClass.equals(ARGODB_DRIVER) && selected.equals("io.transwarp.hive.jdbc.HiveDriver")))
      throw new SQLException("DriverManager selected a different product driver: "+selected);
    Properties props=new Properties();
    if(!driverClass.equals(SPANNER_DRIVER))props.setProperty("user","navkitRuntimeProbe");
    driver.getPropertyInfo(url,props);
    if(driver.connect("jdbc:navkit-unrecognised:",props)!=null)throw new SQLException("Driver accepted a foreign JDBC URL");
    if(driverClass.equals("io.trino.jdbc.TrinoDriver") || driverClass.equals("io.prestosql.jdbc.PrestoDriver") || driverClass.equals("org.h2.Driver")) {
      try(Connection connection=driver.connect(url,props)) { if(connection==null)throw new SQLException("Product JDBC factory returned null"); }
    }
    System.out.println("NAVKIT_JDBC_READY\t"+driverClass+"\t"+driver.getMajorVersion()+"."+driver.getMinorVersion()+"\t"+Runtime.version().feature());
  }
  public static void main(String[] args) throws Exception {
    if(args.length==3 && args[0].equals("--probe")) { probe(args[1],args[2]); return; }
    BufferedReader in=new BufferedReader(new InputStreamReader(System.in,StandardCharsets.UTF_8));
    PrintWriter out=new PrintWriter(new OutputStreamWriter(System.out,StandardCharsets.UTF_8),true);
    try {
      String line;
      while((line=JdbcMutationSupport.readLine(in))!=null) {
        try {
          String[] p=line.split("\t",-1);
          if(p[0].equals("connect")) {
            if(conn!=null)throw new SQLException("Sidecar already owns a physical connection", "08002");
            driverClass=decode(p[4]);
            String url=decode(p[1]),user=decode(p[2]); password=decode(p[3]);
            if(driverClass.equals(BIGQUERY_DRIVER) || driverClass.equals(SPANNER_DRIVER)) suppressCredentialDriverLogs();
            if(!driverClass.isEmpty())Class.forName(driverClass);
            DriverManager.setLoginTimeout(p.length>5?Integer.parseInt(p[5]):30);
            if(driverClass.equals(BIGQUERY_DRIVER)) {
              if(!user.equals("token") || password.isEmpty() || !password.matches("[A-Za-z0-9._~+/=-]+"))
                throw new SQLException("BigQuery requires a pre-generated OAuth bearer token in the password field", "28000");
              Properties credentials=new Properties();
              credentials.setProperty("OAuthType","2"); credentials.setProperty("OAuthAccessToken",password);
              credentials.setProperty("OAuthAccessTokenReadonly","0"); credentials.setProperty("LogLevel","0");
              credentials.setProperty("EnableDiagnosticTelemetry","0"); credentials.setProperty("enableGcpLogExporter","false");
              credentials.setProperty("enableGcpTraceExporter","false"); credentials.setProperty("useGlobalOpenTelemetry","false");
              conn=DriverManager.getConnection(url,credentials);
            } else if(driverClass.equals(SPANNER_DRIVER)) {
              Properties credentials=new Properties();
              if(!user.isEmpty() || !password.isEmpty()) {
                if(!user.equals("token") || password.isEmpty() || !password.matches("[A-Za-z0-9._~+/=-]+"))
                  throw new SQLException("Spanner requires a pre-generated OAuth token, or empty credentials for an explicit local emulator", "28000");
                if(url.toLowerCase(Locale.ROOT).contains("useplaintext=true"))
                  throw new SQLException("Spanner never sends an OAuth token to a plaintext endpoint", "28000");
                credentials.setProperty("oauthtoken",password);
              } else if(!url.contains(";usePlainText=true")) {
                throw new SQLException("Spanner cloud credentials are required; no implicit ADC search", "28000");
              }
              conn=DriverManager.getConnection(url,credentials);
            } else if(driverClass.equals(ARGODB_DRIVER)) {
              if(!user.isEmpty() || !password.isEmpty())
                throw new SQLException("ArgoDB Quark auth=noSasl route requires empty credentials; configure explicit custom JDBC for extended authentication", "28000");
              Driver exact=(Driver)Class.forName(ARGODB_DRIVER).getDeclaredConstructor().newInstance();
              conn=exact.connect(url,new Properties());
              if(conn==null)throw new SQLException("ArgoDB Quark driver rejected its product URL", "08001");
            } else conn=user.isEmpty() && password.isEmpty()?DriverManager.getConnection(url):DriverManager.getConnection(url,user,password);
            DatabaseMetaData md=conn.getMetaData(); String qs=md.getIdentifierQuoteString(); quote=qs==null || qs.trim().isEmpty()?"":qs;
            qualifiedCatalogs=driverClass.equals("io.trino.jdbc.TrinoDriver") || driverClass.equals("io.prestosql.jdbc.PrestoDriver") || driverClass.equals("com.databricks.client.jdbc.Driver") || driverClass.startsWith("net.snowflake.");
            catalogOnly=!qualifiedCatalogs && !md.supportsSchemasInTableDefinitions() && md.supportsCatalogsInTableDefinitions();
            initialCatalog=catalog(); initialSchema=schema();
            relational=new JdbcMutationSupport(conn,"jdbc");
            rowsJson(out,Collections.singletonList(new String[]{md.getDatabaseProductName(),md.getDatabaseProductVersion()}),new String[]{"product","version"});
          } else if(p[0].equals("v2")) {
            if(relational==null || p.length!=2)throw new SQLException("Relational sidecar is not connected", "08003");
            relational.handle(p[1],out,password);
          } else if(p[0].equals("ping")) { ping(out,Integer.parseInt(p[1])); }
          else if(p[0].equals("query")) { query(out,decode(p[1]),Integer.parseInt(p[2]),p.length>3?decode(p[3]):""); }
          else if(p[0].equals("containers")) { containers(out); }
          else if(p[0].equals("tables")) { tables(out,decode(p[1])); }
          else if(p[0].equals("rows")) { rows(out,decode(p[1]),decode(p[2]),Long.parseLong(p[3]),Long.parseLong(p[4]),p.length>5?Integer.parseInt(p[5]):30); }
          else if(p[0].equals("columns")) { columns(out,decode(p[1]),decode(p[2])); }
          else throw new IllegalArgumentException("Unknown sidecar command");
        } catch(Exception e) { error(out,e); }
      }
    } finally { if(conn!=null && !conn.isClosed())conn.close(); }
  }
}
