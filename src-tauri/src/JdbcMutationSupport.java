// Appended to each source-launcher compilation unit; no external JSON dependency.
// Java 11/JDBC APIs remain the transport; the certified embedded H2 plugin
// inspects version-gated actual schema object identity to reject DB_OBJECT_ID ABA.
final class JdbcMutationSupport {
  static final int PROTOCOL = 2, FRAME_LIMIT = 48 * 1024 * 1024, PAYLOAD_LIMIT = 32 * 1024 * 1024;
  static final int VALUE_LIMIT = 16 * 1024 * 1024, COLUMN_LIMIT = 1024;
  final Connection connection;
  final String adapterId, sessionId = UUID.randomUUID().toString();
  boolean poisoned;
  static final int OBJECT_GENERATION_LIMIT=4096;
  static final class ObjectGeneration {
    final java.lang.ref.WeakReference<Object> object;
    final String generation=UUID.randomUUID().toString();
    ObjectGeneration(Object object) { this.object=new java.lang.ref.WeakReference<>(object); }
  }
  final List<ObjectGeneration> objectGenerations=new ArrayList<>();

  JdbcMutationSupport(Connection connection, String adapterId) {
    this.connection = connection; this.adapterId = adapterId;
  }

  static String readLine(BufferedReader input) throws IOException {
    StringBuilder line = new StringBuilder();
    int c;
    while ((c = input.read()) != -1) {
      if (c == '\n') return line.toString();
      if (line.length() >= FRAME_LIMIT) throw new IOException("Sidecar frame exceeds 48 MiB");
      if (c != '\r') line.append((char)c);
    }
    if (line.length() != 0) throw new EOFException("Incomplete sidecar frame");
    return null;
  }

  static String text(Object value) throws SQLException {
    if (!(value instanceof String)) throw fault("INVALID_REQUEST", "22023", "Expected a string");
    return (String)value;
  }
  static Map<String,Object> object(Object value) throws SQLException {
    if (!(value instanceof Map)) throw fault("INVALID_REQUEST", "22023", "Expected an object");
    @SuppressWarnings("unchecked") Map<String,Object> result = (Map<String,Object>)value;
    return result;
  }
  static List<?> array(Object value) throws SQLException {
    if (!(value instanceof List)) throw fault("INVALID_REQUEST", "22023", "Expected an array");
    return (List<?>)value;
  }
  static boolean bool(Object value) throws SQLException {
    if (!(value instanceof Boolean)) throw fault("INVALID_REQUEST", "22023", "Expected a boolean");
    return (Boolean)value;
  }
  static int integer(Object value, int min, int max) throws SQLException {
    if (!(value instanceof java.math.BigDecimal)) throw fault("INVALID_REQUEST", "22023", "Expected an integer");
    try { int n = ((java.math.BigDecimal)value).intValueExact(); if (n < min || n > max) throw new ArithmeticException(); return n; }
    catch (ArithmeticException error) { throw fault("INVALID_REQUEST", "22023", "Integer outside safe range"); }
  }
  static Map<String,Object> map(Object... values) {
    Map<String,Object> result = new LinkedHashMap<>();
    for (int i=0; i<values.length; i+=2) result.put((String)values[i], values[i+1]);
    return result;
  }
  static SQLException fault(String code, String state, String message) { return new OperationException(code, state, message); }
  static final class OperationException extends SQLException {
    final String code;
    OperationException(String code, String state, String message) { super(message, state); this.code = code; }
  }

  static String encode(Object value) {
    if (value == null) return "null";
    if (value instanceof String) return NavkitJsonString.quote((String)value);
    if (value instanceof Boolean || value instanceof Number) return value.toString();
    StringBuilder out = new StringBuilder();
    if (value instanceof Map) {
      out.append('{'); boolean first=true;
      for (Map.Entry<?,?> entry:((Map<?,?>)value).entrySet()) {
        if (!first) out.append(','); first=false;
        out.append(NavkitJsonString.quote((String)entry.getKey())).append(':').append(encode(entry.getValue()));
      }
      return out.append('}').toString();
    }
    if (value instanceof List) {
      out.append('['); boolean first=true;
      for (Object item:(List<?>)value) { if(!first)out.append(','); first=false; out.append(encode(item)); }
      return out.append(']').toString();
    }
    throw new IllegalArgumentException("Unsupported JSON value");
  }

  void handle(String encoded, PrintWriter output, String secret) {
    String requestId="", requestedSession="";
    Map<String,Object> envelope;
    try {
      byte[] bytes=Base64.getDecoder().decode(encoded);
      if (bytes.length > PAYLOAD_LIMIT || !Base64.getEncoder().encodeToString(bytes).equals(encoded))
        throw fault("INVALID_REQUEST", "54000", "Noncanonical or oversized v2 payload");
      String json=StandardCharsets.UTF_8.newDecoder().onMalformedInput(java.nio.charset.CodingErrorAction.REPORT)
        .onUnmappableCharacter(java.nio.charset.CodingErrorAction.REPORT).decode(java.nio.ByteBuffer.wrap(bytes)).toString();
      Map<String,Object> request=object(new JsonReader(json).parse());
      only(request, "protocol", "request_id", "session_id", "action", "payload");
      requestId=text(request.get("request_id")); UUID.fromString(requestId);
      requestedSession=text(request.get("session_id"));
      if(integer(request.get("protocol"),2,2)!=PROTOCOL)throw fault("PROTOCOL_MISMATCH","08006","Sidecar v2 required");
      String action=text(request.get("action"));
      if ((!action.equals("capabilities") || !requestedSession.isEmpty()) && !sessionId.equals(requestedSession))
        throw fault("STALE_CONTEXT","08006","Wrong physical JDBC session; re-preview after reconnect");
      if(poisoned || connection.isClosed())throw fault("CONNECTION_POISONED","08003","Physical connection was discarded; reconnect required");
      Map<String,Object> payload=object(request.get("payload"));
      Object result;
      switch(action) {
        case "capabilities": only(payload); result=capabilities(); break;
        case "metadata": only(payload,"object"); result=metadataScoped(object(payload.get("object"))); break;
        case "row_plan": result=mutate(payload); break;
        default: throw fault("UNSUPPORTED_CAPABILITY","0A000","Unsupported relational operation");
      }
      envelope=map("protocol",PROTOCOL,"request_id",requestId,"session_id",sessionId,"result",result);
    } catch(Exception error) {
      String detail=error.getMessage()==null?error.getClass().getSimpleName():error.getMessage();
      if(secret!=null && !secret.isEmpty())detail=detail.replace(secret,"[REDACTED]");
      // Driver errors can include bound row values: do not echo them through IPC/logs.
      String code=error instanceof OperationException?((OperationException)error).code:"DRIVER_ERROR";
      if(!(error instanceof OperationException))detail=error.getClass().getSimpleName()+"; relational operation failed";
      envelope=map("protocol",PROTOCOL,"request_id",requestId,"session_id",sessionId,"error",map(
        "code",code,"sql_state",error instanceof SQLException?Objects.toString(((SQLException)error).getSQLState(),""):"",
        "message",detail,"effects",poisoned?"unknown":"none","connection_poisoned",poisoned));
    }
    String response=encode(envelope);
    if(response.getBytes(StandardCharsets.UTF_8).length>FRAME_LIMIT) {
      poison(); response=encode(map("protocol",PROTOCOL,"request_id",requestId,"session_id",sessionId,"error",map(
        "code","RESULT_TOO_LARGE","sql_state","54000","message","Response exceeded frame limit; outcome must be reconciled",
        "effects","unknown","connection_poisoned",true)));
    }
    output.println(response);
    if(output.checkError())poison();
  }

  static void only(Map<String,Object> map, String... names) throws SQLException {
    Set<String> allowed=new HashSet<>(Arrays.asList(names));
    if(!map.keySet().equals(allowed))throw fault("INVALID_REQUEST","22023","Missing or unexpected request fields");
  }
  void poison() {
    poisoned=true;
    try { connection.close(); } catch(Exception ignored) { }
  }

  interface MetadataText { String get() throws SQLException; }
  static String optionalMetadataText(MetadataText getter) throws SQLException {
    try { return Objects.toString(getter.get(),""); }
    catch(SQLFeatureNotSupportedException | AbstractMethodError unsupported) { return ""; }
  }
  Map<String,Object> capabilities() throws SQLException {
    DatabaseMetaData md=connection.getMetaData();
    String product=md.getDatabaseProductName(), productVersion=md.getDatabaseProductVersion();
    String driverName=optionalMetadataText(md::getDriverName), driverVersion=optionalMetadataText(md::getDriverVersion);
    boolean h2="H2".equals(product) && "2.3.232 (2024-08-11)".equals(productVersion)
      && driverName.startsWith("H2 JDBC Driver") && driverVersion.startsWith("2.3.232");
    String reason=null;
    if(h2) {
      try(PreparedStatement query=connection.prepareStatement("SELECT SETTING_VALUE FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME='MODE'")) {
        query.setQueryTimeout(5);
        try(ResultSet rows=query.executeQuery()) { h2=rows.next() && "REGULAR".equals(rows.getString(1)) && !rows.next(); }
      }
      if(!h2)reason="H2 compatibility modes have not been certified for typed mutations";
    } else reason="No certified relational mutation plugin for this product/driver version";
    if(h2) {
      try { embeddedSession(); }
      catch(SQLException unavailable) { h2=false;reason="Reliable physical schema-object generation is unavailable; only embedded H2 mem/file transports are certified"; }
    }
    // Editing capability discovery cannot break an existing read-only warehouse
    // merely because it lacks transaction APIs. Unknown write status is conservatively
    // read-only, with row_write=false and a reason; it is never assumed writable.
    boolean transactions=h2 && md.supportsTransactions() && md.supportsTransactionIsolationLevel(Connection.TRANSACTION_SERIALIZABLE);
    boolean readOnly=true;
    try { readOnly=connection.isReadOnly(); } catch(SQLFeatureNotSupportedException | AbstractMethodError unsupported) { }
    if(h2 && !transactions)reason="Driver cannot provide the required owned SERIALIZABLE transaction";
    if(h2 && readOnly)reason="Physical connection is read-only";
    if(h2 && !connection.getAutoCommit())reason="Physical connection is already owned by a manual transaction";
    boolean writable=h2 && transactions && !readOnly && connection.getAutoCommit();
    return map("protocol_version",PROTOCOL,"session_id",sessionId,"adapter_id",adapterId,"product",product,
      "product_version",productVersion,"driver_name",driverName,"driver_version",driverVersion,
      "catalog",optionalMetadataText(connection::getCatalog),"schema",optionalMetadataText(connection::getSchema),
      "read_only",readOnly,"transactions",transactions,"metadata",h2,"row_write",writable,"ddl_write",false,
      "bound_types",h2?Arrays.asList("null","text","integer","decimal","float","bool","binary","date","time","timestamp","timestamptz","uuid"):Collections.emptyList(),
      "reason",reason);
  }

  Map<String,Object> objectId(Map<String,Object> value) throws SQLException {
    only(value,"catalog","schema","name");
    for(String key:Arrays.asList("catalog","schema","name")) {
      String name=text(value.get(key));
      if(name.isEmpty() || name.length()>1024 || name.indexOf(0)>=0)throw fault("INVALID_OBJECT","42000","Invalid complete object identity");
    }
    if(!Objects.equals(connection.getCatalog(),value.get("catalog")))throw fault("STALE_CONTEXT","3D000","Object catalog does not match the physical connection");
    return new LinkedHashMap<>(value);
  }
  String quote(String value) throws SQLException {
    String quote=connection.getMetaData().getIdentifierQuoteString();
    if(quote==null || quote.trim().isEmpty())throw fault("UNSUPPORTED_CAPABILITY","0A000","Safe identifier quoting unavailable");
    return quote+value.replace(quote,quote+quote)+quote;
  }
  String table(Map<String,Object> object) throws SQLException { return quote(text(object.get("schema")))+"."+quote(text(object.get("name"))); }
  void select(Map<String,Object> object) throws SQLException {
    objectId(object); connection.setSchema(text(object.get("schema")));
    if(!Objects.equals(connection.getSchema(),object.get("schema")))throw fault("STALE_CONTEXT","3F000","Driver did not select requested schema");
  }
  Map<String,Object> metadataScoped(Map<String,Object> id) throws SQLException {
    Map<String,Object> caps=capabilities();
    if(!bool(caps.get("metadata")))throw fault("UNSUPPORTED_CAPABILITY","0A000",text(caps.get("reason")));
    String previous=connection.getSchema();
    try { select(id); return metadata(id); }
    finally { try { connection.setSchema(previous); if(!Objects.equals(connection.getSchema(),previous))throw new SQLException(); }
      catch(SQLException error) { poison(); throw fault("CONNECTION_POISONED","08006","Failed to restore the physical namespace"); } }
  }
  // Numeric H2 object IDs are recycled after DROP+CREATE, even for identical DDL.
  // A bounded weak identity token belongs to the actual embedded Table object,
  // not a row counter, wall clock or a second execution/authorization registry.
  Object embeddedSession() throws SQLException {
    try {
      Class<?> jdbc=Class.forName("org.h2.jdbc.JdbcConnection");
      Object actual=connection.unwrap(jdbc), session=jdbc.getMethod("getSession").invoke(actual);
      if(!Class.forName("org.h2.engine.SessionLocal").isInstance(session))
        throw fault("UNSUPPORTED_CAPABILITY","0A000","Remote H2 schema-object generation is not certified");
      return session;
    } catch(ReflectiveOperationException | LinkageError error) {
      throw fault("UNSUPPORTED_CAPABILITY","0A000","Actual embedded H2 schema-object identity is unavailable");
    }
  }
  String objectGeneration(Map<String,Object> id,String backendId) throws SQLException {
    try {
      Object session=embeddedSession(), database=session.getClass().getMethod("getDatabase").invoke(session);
      Object schema=database.getClass().getMethod("findSchema",String.class).invoke(database,text(id.get("schema")));
      if(schema==null)throw fault("METADATA_CHANGED","40001","Actual schema disappeared while reading metadata");
      Object table=schema.getClass().getMethod("findTableOrView",Class.forName("org.h2.engine.SessionLocal"),String.class)
        .invoke(schema,session,text(id.get("name")));
      if(table==null || !backendId.equals(String.valueOf(table.getClass().getMethod("getId").invoke(table))))
        throw fault("METADATA_CHANGED","40001","Actual schema-object identity changed while reading metadata");
      objectGenerations.removeIf(entry->entry.object.get()==null);
      for(ObjectGeneration entry:objectGenerations)if(entry.object.get()==table)return entry.generation;
      if(objectGenerations.size()>=OBJECT_GENERATION_LIMIT)
        throw fault("METADATA_UNKNOWN","54000","Bounded schema-object generation cache is full; re-open the connection");
      ObjectGeneration entry=new ObjectGeneration(table);objectGenerations.add(entry);return entry.generation;
    } catch(ReflectiveOperationException | LinkageError error) {
      throw fault("METADATA_UNKNOWN","0A000","Reliable actual H2 schema-object generation could not be read");
    }
  }

  static String pattern(DatabaseMetaData md, String value) throws SQLException {
    String escape=md.getSearchStringEscape();
    return value.replace(escape,escape+escape).replace("%",escape+"%").replace("_",escape+"_");
  }
  static Boolean yesNo(String value) { return "YES".equals(value)?Boolean.TRUE:"NO".equals(value)?Boolean.FALSE:null; }
  static List<String> ordered(Map<Integer,String> items) throws SQLException {
    List<String> result=new ArrayList<>();
    for(int i=1;i<=items.size();i++) { String value=items.get(i); if(value==null)throw fault("METADATA_UNKNOWN","0A000","Incomplete ordered key metadata"); result.add(value); }
    return result;
  }
  static boolean writableType(int type, String name) {
    if("UUID".equals(name))return true;
    if(name.contains("IGNORECASE"))return false;
    switch(type) {
      case Types.TINYINT:case Types.SMALLINT:case Types.INTEGER:case Types.BIGINT:
      case Types.NUMERIC:case Types.DECIMAL:case Types.REAL:case Types.FLOAT:case Types.DOUBLE:
      case Types.BOOLEAN:case Types.BIT:case Types.CHAR:case Types.VARCHAR:case Types.NCHAR:case Types.NVARCHAR:
      case Types.BINARY:case Types.VARBINARY:case Types.DATE:case Types.TIME:case Types.TIMESTAMP:case Types.TIMESTAMP_WITH_TIMEZONE:return true;
      default:return false;
    }
  }

  Map<String,Object> metadata(Map<String,Object> id) throws SQLException {
    DatabaseMetaData md=connection.getMetaData(); String catalog=text(id.get("catalog")), schema=text(id.get("schema")), name=text(id.get("name"));
    String type=null;
    try(ResultSet rows=md.getTables(catalog,pattern(md,schema),pattern(md,name),new String[]{"BASE TABLE","TABLE","VIEW"})) {
      while(rows.next()) { if(catalog.equals(rows.getString("TABLE_CAT")) && schema.equals(rows.getString("TABLE_SCHEM")) && name.equals(rows.getString("TABLE_NAME"))) {
        if(type!=null)throw fault("METADATA_UNKNOWN","0A000","Ambiguous table identity"); type=rows.getString("TABLE_TYPE");
      } }
    }
    if(type==null)throw fault("OBJECT_NOT_FOUND","42S02","Target object is absent or inaccessible");
    List<Map<String,Object>> columns=new ArrayList<>();
    try(ResultSet rows=md.getColumns(catalog,pattern(md,schema),pattern(md,name),"%")) {
      while(rows.next()) {
        if(!catalog.equals(rows.getString("TABLE_CAT")) || !schema.equals(rows.getString("TABLE_SCHEM")) || !name.equals(rows.getString("TABLE_NAME")))continue;
        String column=rows.getString("COLUMN_NAME"); int jdbcType=rows.getInt("DATA_TYPE"), nullable=rows.getInt("NULLABLE");
        String typeName=rows.getString("TYPE_NAME");
        columns.add(map("name",column,"jdbc_type",jdbcType,"type_name",typeName,"ordinal",rows.getInt("ORDINAL_POSITION"),
          "precision",rows.getInt("COLUMN_SIZE"),"scale",rows.getInt("DECIMAL_DIGITS"),
          "nullable",nullable==DatabaseMetaData.columnNoNulls?Boolean.FALSE:nullable==DatabaseMetaData.columnNullable?Boolean.TRUE:null,
          "auto_increment",yesNo(rows.getString("IS_AUTOINCREMENT")),"generated",yesNo(rows.getString("IS_GENERATEDCOLUMN")),
          "read_only",false,"default_value",rows.getString("COLUMN_DEF"),"generation_expression",null,"writable_type",writableType(jdbcType,typeName)));
        if(columns.size()>COLUMN_LIMIT)throw fault("METADATA_UNKNOWN","54000","Table exceeds 1024-column limit");
      }
    }
    columns.sort(Comparator.comparingInt(c->((Number)c.get("ordinal")).intValue()));
    if(columns.isEmpty())throw fault("METADATA_UNKNOWN","0A000","No complete column metadata");
    Set<String> columnNames=new HashSet<>();
    for(int i=0;i<columns.size();i++)if(((Number)columns.get(i).get("ordinal")).intValue()!=i+1 || !columnNames.add((String)columns.get(i).get("name")))
      throw fault("METADATA_UNKNOWN","0A000","Column order or identity is incomplete");
    try(PreparedStatement query=connection.prepareStatement("SELECT * FROM "+table(id)+" WHERE 1=0"); ResultSet empty=query.executeQuery()) {
      ResultSetMetaData meta=empty.getMetaData();
      if(meta.getColumnCount()!=columns.size())throw fault("METADATA_CHANGED","40001","Column metadata changed concurrently");
      for(int i=0;i<columns.size();i++) {
        Map<String,Object> column=columns.get(i);
        if(!Objects.equals(meta.getColumnName(i+1),column.get("name")))throw fault("METADATA_CHANGED","40001","Column metadata changed concurrently");
        column.put("read_only",meta.isReadOnly(i+1) || !meta.isWritable(i+1));
      }
    }
    Map<Integer,String> primary=new TreeMap<>();
    try(ResultSet rows=md.getPrimaryKeys(catalog,schema,name)) {
      while(rows.next()) { String column=rows.getString("COLUMN_NAME");
        if(!columnNames.contains(column) || primary.put(rows.getInt("KEY_SEQ"),column)!=null)throw fault("METADATA_UNKNOWN","0A000","Invalid primary-key metadata"); }
    }
    List<String> primaryKey=ordered(primary); List<List<String>> uniqueKeys=new ArrayList<>();
    if(!primaryKey.isEmpty())uniqueKeys.add(primaryKey);
    List<Map<String,Object>> indexes=new ArrayList<>(); Map<String,Map<Integer,String>> unique=new TreeMap<>(); Set<String> incomplete=new HashSet<>();
    try(ResultSet rows=md.getIndexInfo(catalog,schema,name,false,false)) {
      while(rows.next()) {
        if(rows.getShort("TYPE")==DatabaseMetaData.tableIndexStatistic)continue;
        String index=rows.getString("INDEX_NAME"), column=rows.getString("COLUMN_NAME"), filter=rows.getString("FILTER_CONDITION");
        int ordinal=rows.getInt("ORDINAL_POSITION"); boolean nonUnique=rows.getBoolean("NON_UNIQUE");
        indexes.add(map("name",index,"column",column,"ordinal",ordinal,"non_unique",nonUnique,"direction",rows.getString("ASC_OR_DESC"),"filter",filter));
        if(!nonUnique && index!=null) {
          if(column==null || !columnNames.contains(column) || filter!=null)incomplete.add(index);
          else if(unique.computeIfAbsent(index,key->new TreeMap<>()).put(ordinal,column)!=null)incomplete.add(index);
        }
      }
    }
    indexes.sort(Comparator.comparing(JdbcMutationSupport::encode));
    for(Map.Entry<String,Map<Integer,String>> entry:unique.entrySet())if(!incomplete.contains(entry.getKey())) {
      List<String> key=ordered(entry.getValue()); if(!key.isEmpty() && !uniqueKeys.contains(key))uniqueKeys.add(key);
    }
    List<Map<String,Object>> foreignKeys=new ArrayList<>();
    try(ResultSet rows=md.getImportedKeys(catalog,schema,name)) {
      while(rows.next())foreignKeys.add(map("name",rows.getString("FK_NAME"),"sequence",rows.getInt("KEY_SEQ"),
        "column",rows.getString("FKCOLUMN_NAME"),"ref_catalog",rows.getString("PKTABLE_CAT"),"ref_schema",rows.getString("PKTABLE_SCHEM"),
        "ref_table",rows.getString("PKTABLE_NAME"),"ref_column",rows.getString("PKCOLUMN_NAME"),
        "on_update",rows.getInt("UPDATE_RULE"),"on_delete",rows.getInt("DELETE_RULE"),"deferrability",rows.getInt("DEFERRABILITY")));
    }
    foreignKeys.sort(Comparator.comparing(JdbcMutationSupport::encode));
    List<Map<String,Object>> extras=selectMetadata("SELECT COLUMN_NAME,GENERATION_EXPRESSION,IS_VISIBLE,COLUMN_ON_UPDATE,IS_IDENTITY,IDENTITY_GENERATION,IDENTITY_START,IDENTITY_INCREMENT,IDENTITY_MAXIMUM,IDENTITY_MINIMUM,IDENTITY_CYCLE,DEFAULT_ON_NULL FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_CATALOG=? AND TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION",id);
    if(extras.size()!=columns.size())throw fault("METADATA_CHANGED","40001","H2 column snapshot changed concurrently");
    for(int i=0;i<columns.size();i++) {
      Map<String,Object> column=columns.get(i), extra=extras.get(i);
      if(!Objects.equals(column.get("name"),extra.get("COLUMN_NAME")))throw fault("METADATA_CHANGED","40001","H2 column snapshot changed concurrently");
      column.put("generation_expression",extra.get("GENERATION_EXPRESSION"));
      column.put("read_only",Boolean.TRUE.equals(column.get("read_only")) || Boolean.TRUE.equals(column.get("generated")) || "FALSE".equals(extra.get("IS_VISIBLE")));
    }
    List<Map<String,Object>> constraints=selectMetadata("SELECT CONSTRAINT_CATALOG,CONSTRAINT_SCHEMA,CONSTRAINT_NAME,CONSTRAINT_TYPE,DB_OBJECT_SQL('CONSTRAINT',CONSTRAINT_SCHEMA,CONSTRAINT_NAME) AS DEFINITION FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS WHERE TABLE_CATALOG=? AND TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY CONSTRAINT_SCHEMA,CONSTRAINT_NAME",id);
    List<Map<String,Object>> triggers=selectMetadata("SELECT TRIGGER_CATALOG,TRIGGER_SCHEMA,TRIGGER_NAME,EVENT_MANIPULATION,ACTION_ORIENTATION,ACTION_TIMING,IS_ROLLBACK,JAVA_CLASS,QUEUE_SIZE,NO_WAIT,DB_OBJECT_SQL('TRIGGER',TRIGGER_SCHEMA,TRIGGER_NAME) AS DEFINITION FROM INFORMATION_SCHEMA.TRIGGERS WHERE EVENT_OBJECT_CATALOG=? AND EVENT_OBJECT_SCHEMA=? AND EVENT_OBJECT_TABLE=? ORDER BY TRIGGER_SCHEMA,TRIGGER_NAME,EVENT_MANIPULATION",id);
    String backendId,definition;
    try(PreparedStatement query=connection.prepareStatement("SELECT DB_OBJECT_ID('TABLE',?,?),DB_OBJECT_SQL('TABLE',?,?)")) {
      query.setString(1,schema);query.setString(2,name);query.setString(3,schema);query.setString(4,name);
      try(ResultSet rows=query.executeQuery()) { if(!rows.next() || rows.getString(1)==null || rows.getString(2)==null)throw fault("METADATA_UNKNOWN","0A000","H2 object identity/definition unavailable");
        backendId=rows.getString(1);definition=rows.getString(2); }
    }
    Map<String,Object> snapshot=map("object",new LinkedHashMap<>(id),"session_id",sessionId,"backend_object_id",backendId,
      "table_type",type,"definition",definition,"columns",columns,"primary_key",primaryKey,"unique_keys",uniqueKeys,
      "indexes",indexes,"foreign_keys",foreignKeys,"constraints",constraints,"triggers",triggers,
      "completeness",map("columns","known","keys","known","indexes","known","foreign_keys","known","constraints","known","triggers","known","definition","known"));
    // Identity base/cardinality/LAST_MODIFICATION are row data, not structure revisions.
    Map<String,Object> fingerprint=new LinkedHashMap<>(snapshot); fingerprint.put("h2_column_details",extras);
    fingerprint.put("actual_object_generation",objectGeneration(id,backendId));
    snapshot.put("revision",digest(encode(fingerprint)));
    return snapshot;
  }

  List<Map<String,Object>> selectMetadata(String sql,Map<String,Object> id) throws SQLException {
    List<Map<String,Object>> result=new ArrayList<>();
    try(PreparedStatement query=connection.prepareStatement(sql)) {
      query.setQueryTimeout(10);query.setString(1,text(id.get("catalog")));query.setString(2,text(id.get("schema")));query.setString(3,text(id.get("name")));
      try(ResultSet rows=query.executeQuery()) { ResultSetMetaData meta=rows.getMetaData();
        while(rows.next()) { Map<String,Object> row=new LinkedHashMap<>(); for(int i=1;i<=meta.getColumnCount();i++)row.put(meta.getColumnLabel(i),rows.getString(i)); result.add(row);
          if(result.size()>4096)throw fault("METADATA_UNKNOWN","54000","Metadata result exceeds bounded snapshot"); }
      }
    }
    return result;
  }
  static String digest(String value) throws SQLException {
    try { byte[] bytes=java.security.MessageDigest.getInstance("SHA-256").digest(value.getBytes(StandardCharsets.UTF_8));
      StringBuilder result=new StringBuilder();for(byte b:bytes)result.append(String.format("%02x",b&255));return result.toString(); }
    catch(java.security.NoSuchAlgorithmException error) { throw new SQLException("SHA-256 unavailable",error); }
  }

  static Map<String,Object> dbValue(String kind,Object value) { return value==null?map("kind","null"):map("kind",kind,"value",value); }
  static Map<String,Object> column(Map<String,Object> metadata,String name) throws SQLException {
    for(Object item:(List<?>)metadata.get("columns")) { Map<String,Object> column=object(item);if(name.equals(column.get("name")))return column; }
    throw fault("INVALID_COLUMN","42S22","Target column does not exist");
  }
  static int jdbcType(Map<String,Object> column) { return ((Number)column.get("jdbc_type")).intValue(); }
  static Map<String,Object> normalize(Map<String,Object> value,Map<String,Object> column) throws SQLException {
    String kind=text(value.get("kind"));only(value,kind.equals("null")?new String[]{"kind"}:new String[]{"kind","value"});
    if(!Boolean.TRUE.equals(column.get("writable_type")))throw fault("UNSUPPORTED_TYPE","0A000","Target type has not been validated for lossless binding");
    if(kind.equals("null"))return dbValue("null",null);
    Object raw=value.get("value");String typeName=(String)column.get("type_name");int type=jdbcType(column);
    if("UUID".equals(typeName)) { if(!kind.equals("uuid"))throw fault("TYPE_MISMATCH","22005","UUID requires uuid tag");
      try { return dbValue("uuid",UUID.fromString(text(raw)).toString()); } catch(IllegalArgumentException error) { throw fault("INVALID_VALUE","22018","Invalid UUID"); } }
    try {
      switch(type) {
        case Types.TINYINT:case Types.SMALLINT:case Types.INTEGER:case Types.BIGINT: {
          String decimal;
          if(kind.equals("int")) { if(!(raw instanceof java.math.BigDecimal))throw new ArithmeticException(); java.math.BigInteger number=((java.math.BigDecimal)raw).toBigIntegerExact();
            if(number.abs().compareTo(new java.math.BigInteger("9007199254740991"))>0)throw new ArithmeticException();decimal=number.toString(); }
          else if(kind.equals("integer"))decimal=text(raw);else throw fault("TYPE_MISMATCH","22005","Integer requires decimal string tag");
          if(decimal.length()>21 || !decimal.matches("[+-]?[0-9]+"))throw new ArithmeticException();
          java.math.BigInteger number=new java.math.BigInteger(decimal);long n=number.longValueExact();
          if(type==Types.TINYINT && (n<Byte.MIN_VALUE || n>Byte.MAX_VALUE) || type==Types.SMALLINT && (n<Short.MIN_VALUE || n>Short.MAX_VALUE) || type==Types.INTEGER && (n<Integer.MIN_VALUE || n>Integer.MAX_VALUE))throw new ArithmeticException();
          return dbValue("integer",number.toString());
        }
        case Types.DECIMAL:case Types.NUMERIC: {
          if(!kind.equals("decimal"))throw fault("TYPE_MISMATCH","22005","DECIMAL requires decimal string tag");String text=text(raw);
          if(text.length()>1024 || !text.matches("[+-]?(?:[0-9]+(?:\\.[0-9]*)?|\\.[0-9]+)(?:[eE][+-]?[0-9]{1,4})?"))throw new ArithmeticException();
          java.math.BigDecimal number=new java.math.BigDecimal(text);if(Math.abs((long)number.scale())>1000)throw new ArithmeticException();
          number=number.setScale(((Number)column.get("scale")).intValue(),java.math.RoundingMode.UNNECESSARY);
          if(number.precision()>((Number)column.get("precision")).intValue())throw new ArithmeticException();
          return dbValue("decimal",number.toPlainString());
        }
        case Types.REAL:case Types.FLOAT:case Types.DOUBLE: {
          if(!kind.equals("float") || !(raw instanceof Number))throw fault("TYPE_MISMATCH","22005","Floating column requires float tag");
          double number=((Number)raw).doubleValue();if(!Double.isFinite(number))throw new ArithmeticException();
          if(type==Types.REAL && (double)(float)number!=number)throw fault("PRECISION_LOSS","22003","Value is not exactly representable as REAL");
          return dbValue("float",number);
        }
        case Types.BOOLEAN:case Types.BIT:if(!kind.equals("bool"))throw fault("TYPE_MISMATCH","22005","Boolean requires bool tag");return dbValue("bool",bool(raw));
        case Types.CHAR:case Types.VARCHAR:case Types.NCHAR:case Types.NVARCHAR: {
          if(!kind.equals("text"))throw fault("TYPE_MISMATCH","22005","Text column requires text tag");String text=text(raw);
          if(text.getBytes(StandardCharsets.UTF_8).length>VALUE_LIMIT || text.length()>((Number)column.get("precision")).intValue())throw new ArithmeticException();
          return dbValue("text",text);
        }
        case Types.BINARY:case Types.VARBINARY: {
          if(!kind.equals("binary"))throw fault("TYPE_MISMATCH","22005","Binary column requires base64 binary tag");String base64=text(raw);
          if(base64.length()>VALUE_LIMIT)throw new ArithmeticException();byte[] bytes=Base64.getDecoder().decode(base64);
          if(!Base64.getEncoder().encodeToString(bytes).equals(base64) || bytes.length>((Number)column.get("precision")).intValue())throw new ArithmeticException();
          return dbValue("binary",base64);
        }
        case Types.DATE:if(!kind.equals("date"))throw fault("TYPE_MISMATCH","22005","Date requires date tag");return dbValue("date",java.time.LocalDate.parse(text(raw)).toString());
        case Types.TIME:if(!kind.equals("time"))throw fault("TYPE_MISMATCH","22005","Time requires time tag");return dbValue("time",java.time.LocalTime.parse(text(raw)).toString());
        case Types.TIMESTAMP:if(!kind.equals("timestamp"))throw fault("TYPE_MISMATCH","22005","Timestamp requires timestamp tag");return dbValue("timestamp",java.time.LocalDateTime.parse(text(raw).replace(' ','T')).toString());
        case Types.TIMESTAMP_WITH_TIMEZONE:if(!kind.equals("timestamptz"))throw fault("TYPE_MISMATCH","22005","Time-zone timestamp requires timestamptz tag");return dbValue("timestamptz",java.time.OffsetDateTime.parse(text(raw).replace(' ','T')).toString());
        default:throw fault("UNSUPPORTED_TYPE","0A000","No validated lossless binding for target type");
      }
    } catch(IllegalArgumentException | ArithmeticException | java.time.DateTimeException error) { throw fault("INVALID_VALUE","22003","Typed value is malformed, out of range or would lose precision"); }
  }

  static Map<String,Map<String,Object>> pairs(Object value,Map<String,Object> metadata) throws SQLException {
    Map<String,Map<String,Object>> result=new LinkedHashMap<>();
    for(Object item:array(value)) { List<?> pair=array(item);if(pair.size()!=2)throw fault("INVALID_REQUEST","22023","Invalid name/value pair");String name=text(pair.get(0));
      if(result.containsKey(name))throw fault("INVALID_COLUMN","42S22","Duplicate binding column");result.put(name,normalize(object(pair.get(1)),column(metadata,name))); }
    if(result.size()>COLUMN_LIMIT)throw fault("INVALID_REQUEST","54000","Too many binding columns");return result;
  }
  static boolean same(Map<String,Object> left,Map<String,Object> right) {
    return left.equals(right);
  }
  static void merge(Map<String,Map<String,Object>> target,Map<String,Map<String,Object>> values) throws SQLException {
    for(Map.Entry<String,Map<String,Object>> entry:values.entrySet()) {
      if(target.containsKey(entry.getKey()) && !same(target.get(entry.getKey()),entry.getValue()))throw fault("CONFLICT","40001","Conflicting original values");
      target.put(entry.getKey(),entry.getValue());
    }
  }
  static boolean isNull(Map<String,Object> value) { return "null".equals(value.get("kind")); }
  static Map<String,Map<String,Object>> key(Map<String,Object> metadata,Map<String,Map<String,Object>> values) throws SQLException {
    for(Object item:array(metadata.get("unique_keys"))) {
      List<?> candidate=array(item);Map<String,Map<String,Object>> result=new LinkedHashMap<>();
      for(Object name:candidate)if(values.containsKey(name) && !isNull(values.get(name)))result.put(text(name),values.get(name));
      if(!result.isEmpty() && result.size()==candidate.size())return result;
    }
    throw fault("MISSING_UNIQUE_KEY","0A000","Complete non-null primary or unique key required");
  }
  static List<Object> serializedPairs(Map<String,Map<String,Object>> values) {
    List<Object> result=new ArrayList<>();for(Map.Entry<String,Map<String,Object>> entry:values.entrySet())result.add(Arrays.asList(entry.getKey(),entry.getValue()));return result;
  }
  String where(Map<String,Map<String,Object>> values) throws SQLException {
    List<String> parts=new ArrayList<>();for(Map.Entry<String,Map<String,Object>> entry:values.entrySet())parts.add(quote(entry.getKey())+(isNull(entry.getValue())?" IS NULL":" = ?"));
    if(parts.isEmpty())throw fault("MISSING_UNIQUE_KEY","0A000","Empty predicate refused");return String.join(" AND ",parts);
  }
  static int bind(PreparedStatement query,int position,Map<String,Map<String,Object>> values,Map<String,Object> metadata) throws SQLException {
    for(Map.Entry<String,Map<String,Object>> entry:values.entrySet())if(!isNull(entry.getValue()))bindValue(query,position++,entry.getValue(),column(metadata,entry.getKey()));return position;
  }
  static void bindValue(PreparedStatement query,int position,Map<String,Object> value,Map<String,Object> column) throws SQLException {
    String kind=(String)value.get("kind");Object raw=value.get("value");
    if(kind.equals("null")) { query.setNull(position,jdbcType(column));return; }
    switch(kind) {
      case "integer":query.setObject(position,new java.math.BigInteger((String)raw).longValueExact(),jdbcType(column));break;
      case "decimal":query.setBigDecimal(position,new java.math.BigDecimal((String)raw));break;
      case "float":query.setDouble(position,((Number)raw).doubleValue());break;
      case "bool":query.setBoolean(position,(Boolean)raw);break;
      case "text":query.setString(position,(String)raw);break;
      case "binary":query.setBytes(position,Base64.getDecoder().decode((String)raw));break;
      case "date":query.setObject(position,java.time.LocalDate.parse((String)raw),Types.DATE);break;
      case "time":query.setObject(position,java.time.LocalTime.parse((String)raw),Types.TIME);break;
      case "timestamp":query.setObject(position,java.time.LocalDateTime.parse((String)raw),Types.TIMESTAMP);break;
      case "timestamptz":query.setObject(position,java.time.OffsetDateTime.parse((String)raw),Types.TIMESTAMP_WITH_TIMEZONE);break;
      case "uuid":query.setObject(position,UUID.fromString((String)raw));break;
      default:throw fault("UNSUPPORTED_TYPE","0A000","Unvalidated typed binder");
    }
  }
  static Map<String,Object> readValue(ResultSet rows,int position,Map<String,Object> column) throws SQLException {
    Object raw;
    if("UUID".equals(column.get("type_name"))) { raw=rows.getObject(position,UUID.class);return dbValue("uuid",raw==null?null:raw.toString()); }
    switch(jdbcType(column)) {
      case Types.TINYINT:case Types.SMALLINT:case Types.INTEGER:case Types.BIGINT:raw=rows.getBigDecimal(position);return dbValue("integer",raw==null?null:((java.math.BigDecimal)raw).toBigIntegerExact().toString());
      case Types.NUMERIC:case Types.DECIMAL:raw=rows.getBigDecimal(position);return dbValue("decimal",raw==null?null:((java.math.BigDecimal)raw).toPlainString());
      case Types.FLOAT:case Types.REAL:case Types.DOUBLE:double number=rows.getDouble(position);return rows.wasNull()?dbValue("null",null):dbValue("float",number);
      case Types.BOOLEAN:case Types.BIT:boolean value=rows.getBoolean(position);return rows.wasNull()?dbValue("null",null):dbValue("bool",value);
      case Types.CHAR:case Types.VARCHAR:case Types.NCHAR:case Types.NVARCHAR:try(Reader stream=rows.getCharacterStream(position)) {
        if(stream==null)return dbValue("null",null);StringBuilder out=new StringBuilder();char[] buffer=new char[8192];int got;
        while((got=stream.read(buffer))!=-1) { if((long)out.length()+got>VALUE_LIMIT)throw fault("RESULT_TOO_LARGE","54000","Readback text exceeds 16 MiB");out.append(buffer,0,got); }
        String text=out.toString();if(text.getBytes(StandardCharsets.UTF_8).length>VALUE_LIMIT)throw fault("RESULT_TOO_LARGE","54000","Readback text exceeds 16 MiB");
        for(int i=0;i<text.length();i++) { char c=text.charAt(i);if(Character.isHighSurrogate(c)) { if(++i>=text.length() || !Character.isLowSurrogate(text.charAt(i)))throw fault("INVALID_VALUE","22021","Readback contains invalid Unicode"); }
          else if(Character.isLowSurrogate(c))throw fault("INVALID_VALUE","22021","Readback contains invalid Unicode"); }
        return dbValue("text",text);
      } catch(IOException error) { throw new SQLException("Text readback failed",error); }
      case Types.BINARY:case Types.VARBINARY:try(InputStream stream=rows.getBinaryStream(position)) {
        if(stream==null)return dbValue("null",null);ByteArrayOutputStream out=new ByteArrayOutputStream();byte[] buffer=new byte[8192];int got;
        while((got=stream.read(buffer))!=-1) { if(out.size()+got>VALUE_LIMIT)throw fault("RESULT_TOO_LARGE","54000","Readback binary exceeds 16 MiB");out.write(buffer,0,got); }
        return dbValue("binary",Base64.getEncoder().encodeToString(out.toByteArray()));
      } catch(IOException error) { throw new SQLException("Binary readback failed",error); }
      case Types.DATE:raw=rows.getObject(position,java.time.LocalDate.class);return dbValue("date",raw==null?null:raw.toString());
      case Types.TIME:raw=rows.getObject(position,java.time.LocalTime.class);return dbValue("time",raw==null?null:raw.toString());
      case Types.TIMESTAMP:raw=rows.getObject(position,java.time.LocalDateTime.class);return dbValue("timestamp",raw==null?null:raw.toString());
      case Types.TIMESTAMP_WITH_TIMEZONE:raw=rows.getObject(position,java.time.OffsetDateTime.class);return dbValue("timestamptz",raw==null?null:raw.toString());
      default:throw fault("UNSUPPORTED_TYPE","0A000","Readback type has not been verified; do not coerce it to text");
    }
  }
  List<Map<String,Map<String,Object>>> readRows(Map<String,Object> id,Map<String,Object> metadata,Map<String,Map<String,Object>> conditions,boolean lock,int timeout) throws SQLException {
    List<Map<String,Object>> readable=new ArrayList<>();List<String> columns=new ArrayList<>();
    for(Object item:array(metadata.get("columns"))) { Map<String,Object> column=object(item);
      if(Boolean.TRUE.equals(column.get("writable_type"))) { readable.add(column);columns.add(quote(text(column.get("name")))); } }
    if(readable.isEmpty())throw fault("UNSUPPORTED_TYPE","0A000","No verified typed readback columns");
    String sql="SELECT "+String.join(",",columns)+" FROM "+table(id)+" WHERE "+where(conditions)+(lock?" FOR UPDATE":"");
    List<Map<String,Map<String,Object>>> result=new ArrayList<>();
    try(PreparedStatement query=connection.prepareStatement(sql)) {
      query.setQueryTimeout(timeout);query.setMaxRows(2);bind(query,1,conditions,metadata);
      try(ResultSet rows=query.executeQuery()) { while(rows.next()) { Map<String,Map<String,Object>> row=new LinkedHashMap<>();int i=1;
        for(Map<String,Object> column:readable)row.put(text(column.get("name")),readValue(rows,i++,column));result.add(row); }
      }
    }
    return result;
  }
  static void verify(Map<String,Map<String,Object>> expected,Map<String,Map<String,Object>> actual) throws SQLException {
    for(Map.Entry<String,Map<String,Object>> entry:expected.entrySet())if(!same(entry.getValue(),actual.get(entry.getKey())))throw fault("CONFLICT","40001","Typed readback differs from planned/original value");
  }

  Map<String,Object> outcome(String operation,String status,String affected,String matched,boolean verified,String transaction,
      List<String> mismatches,Map<String,Map<String,Object>> row,Map<String,Map<String,Object>> generated) {
    return map("operation_id",operation,"status",status,"affected_rows",affected,"matched_rows",matched,"verified",verified,
      "transaction",transaction,"connection_poisoned",poisoned,"mismatches",mismatches,"readback",serializedPairs(row),"generated_keys",serializedPairs(generated));
  }
  Map<String,Object> mutate(Map<String,Object> request) throws SQLException {
    only(request,"operation_id","session_id","object","base_revision","kind","keys","original_values","changes","read_only","timeout_secs");
    String operation=text(request.get("operation_id"));UUID.fromString(operation);
    if(!sessionId.equals(text(request.get("session_id"))))throw fault("STALE_CONTEXT","08006","Plan belongs to another physical session");
    if(bool(request.get("read_only")))throw fault("READ_ONLY_VIOLATION","25006","Saved effective connection is read-only; no SQL sent");
    Map<String,Object> caps=capabilities();if(!bool(caps.get("row_write")))throw fault("UNSUPPORTED_CAPABILITY","0A000",text(caps.get("reason")));
    Map<String,Object> id=objectId(object(request.get("object")));String revision=text(request.get("base_revision")), kind=text(request.get("kind"));
    if(!Arrays.asList("insert","update","delete").contains(kind))throw fault("UNSUPPORTED_CAPABILITY","0A000","Only compiled single-row DML is allowed");
    int timeout=integer(request.get("timeout_secs"),1,300);String previousSchema=connection.getSchema();int previousIsolation=connection.getTransactionIsolation();
    boolean started=false, commitStarted=false, committed=false;String matched=null, affected=null;
    Map<String,Map<String,Object>> returned=new LinkedHashMap<>(), generated=new LinkedHashMap<>();String status="applied";
    Map<String,Object> terminal=null;
    try {
      select(id);connection.setTransactionIsolation(Connection.TRANSACTION_SERIALIZABLE);connection.setAutoCommit(false);started=true;
      Map<String,Object> metadata=metadata(id);
      if(!revision.equals(metadata.get("revision")))throw fault("CONFLICT","40001","Table metadata revision changed; re-preview required");
      if(!"BASE TABLE".equals(metadata.get("table_type")) && !"TABLE".equals(metadata.get("table_type")))throw fault("UNSUPPORTED_CAPABILITY","0A000","Views cannot be edited as base tables");
      Map<String,Map<String,Object>> keys=pairs(request.get("keys"),metadata), original=pairs(request.get("original_values"),metadata), changes=pairs(request.get("changes"),metadata);
      if(kind.equals("update") && changes.isEmpty())throw fault("INVALID_REQUEST","22023","Empty update plan");
      for(Map.Entry<String,Map<String,Object>> entry:changes.entrySet()) {
        Map<String,Object> column=column(metadata,entry.getKey());
        if(Boolean.TRUE.equals(column.get("read_only")) || !Boolean.FALSE.equals(column.get("generated")) || !Boolean.FALSE.equals(column.get("auto_increment")))
          throw fault("READ_ONLY_COLUMN","25006","Generated, identity or unknown-writability column is read-only");
        if(isNull(entry.getValue()) && !Boolean.TRUE.equals(column.get("nullable")))throw fault("INVALID_VALUE","23502","Nonnullable column cannot be assigned NULL");
      }
      Map<String,Map<String,Object>> conditions=new LinkedHashMap<>(keys);merge(conditions,original);
      Map<String,Map<String,Object>> current=null, oldKey=null;
      if(!kind.equals("insert")) {
        oldKey=key(metadata,keys.isEmpty()?original:keys);
        List<Map<String,Map<String,Object>>> found=readRows(id,metadata,conditions,true,timeout);matched=Integer.toString(found.size());
        if(found.size()!=1)throw fault("CONFLICT","40001","Original row does not match exactly one unique target");
        current=found.get(0);verify(conditions,current);
        if(!revision.equals(metadata(id).get("revision")))throw fault("CONFLICT","40001","Metadata changed while acquiring row ownership");
      } else if(array(metadata.get("unique_keys")).isEmpty())throw fault("MISSING_UNIQUE_KEY","0A000","Insert requires a verified key and generated-key readback");
      boolean noOp=false;
      // Evaluate against the actually locked old row (not a client-side equality guess).
      if(kind.equals("update")) { noOp=true;for(Map.Entry<String,Map<String,Object>> entry:changes.entrySet())if(!same(entry.getValue(),current.get(entry.getKey())))noOp=false; }
      if(noOp) { affected="0";status="unchanged";returned=current; }
      else {
        String sql;
        if(kind.equals("insert")) {
          if(changes.isEmpty())sql="INSERT INTO "+table(id)+" DEFAULT VALUES";
          else { List<String> names=new ArrayList<>();for(String name:changes.keySet())names.add(quote(name));sql="INSERT INTO "+table(id)+" ("+String.join(",",names)+") VALUES ("+String.join(",",Collections.nCopies(names.size(),"?"))+")"; }
        } else if(kind.equals("update")) {
          if(changes.isEmpty())throw fault("INVALID_REQUEST","22023","Empty update plan");List<String> sets=new ArrayList<>();for(String name:changes.keySet())sets.add(quote(name)+" = ?");
          sql="UPDATE "+table(id)+" SET "+String.join(",",sets)+" WHERE "+where(conditions);
        } else sql="DELETE FROM "+table(id)+" WHERE "+where(conditions);
        List<?> primary=array(metadata.get("primary_key"));String[] generatedNames=primary.stream().map(String::valueOf).toArray(String[]::new);
        try(PreparedStatement query=kind.equals("insert") && generatedNames.length>0?connection.prepareStatement(sql,generatedNames):connection.prepareStatement(sql)) {
          query.setQueryTimeout(timeout);int position=1;
          if(!kind.equals("delete"))for(Map.Entry<String,Map<String,Object>> entry:changes.entrySet())bindValue(query,position++,entry.getValue(),column(metadata,entry.getKey()));
          if(!kind.equals("insert"))bind(query,position,conditions,metadata);
          // A driver error after execute is never replayed using another execute API.
          long count=query.executeLargeUpdate();
          affected=count>=0?Long.toString(count):null;
          if(count!=1)throw fault("CONFLICT","40001","Driver did not report exactly one affected row; transaction refused");
          if(kind.equals("insert") && generatedNames.length>0)try(ResultSet result=query.getGeneratedKeys()) {
            ResultSetMetaData meta=result.getMetaData();if(result.next()) {
              for(int i=1;i<=meta.getColumnCount();i++) { String name=meta.getColumnLabel(i);if(primary.contains(name))generated.put(name,normalize(readValue(result,i,column(metadata,name)),column(metadata,name))); }
              if(result.next())throw fault("CONFLICT","40001","Generated keys describe more than one row");
            }
          }
        }
        if(kind.equals("delete")) {
          if(!readRows(id,metadata,oldKey,false,timeout).isEmpty())throw fault("CONFLICT","40001","Deleted unique key still exists; trigger/readback mismatch");returned=new LinkedHashMap<>();
        } else {
          Map<String,Map<String,Object>> expectedKeyValues=new LinkedHashMap<>();if(current!=null)expectedKeyValues.putAll(current);expectedKeyValues.putAll(changes);
          if(kind.equals("insert"))merge(expectedKeyValues,generated);
          Map<String,Map<String,Object>> targetKey=key(metadata,expectedKeyValues);
          List<Map<String,Map<String,Object>>> found=readRows(id,metadata,targetKey,false,timeout);
          if(found.size()!=1)throw fault("CONFLICT","40001","New key/readback did not identify exactly one row");returned=found.get(0);verify(targetKey,returned);verify(changes,returned);
          if(kind.equals("insert"))matched="1";
        }
      }
      if(!revision.equals(metadata(id).get("revision")))throw fault("CONFLICT","40001","Table metadata changed before commit");
      // Verify response size BEFORE COMMIT. A successful write cannot become a fake
      // retryable error because the resulting row is too large for the IPC frame.
      if(encode(outcome(operation,status,affected,matched,true,"committed",Collections.emptyList(),returned,generated)).getBytes(StandardCharsets.UTF_8).length>PAYLOAD_LIMIT)
        throw fault("RESULT_TOO_LARGE","54000","Typed readback exceeds 32 MiB; transaction refused");
      commitStarted=true;connection.commit();committed=true;
    } catch(Exception error) {
      if(commitStarted) {
        poison();return terminal=outcome(operation,"unknown",null,matched,false,"unknown",Collections.singletonList("Commit acknowledgement failed; do not replay; reconcile using a new connection"),new LinkedHashMap<>(),new LinkedHashMap<>());
      }
      if(started) {
        try { connection.rollback(); }
        catch(SQLException rollbackError) { poison();return terminal=outcome(operation,"unknown",null,matched,false,"unknown",Collections.singletonList("Rollback acknowledgement failed; connection discarded; do not replay"),new LinkedHashMap<>(),new LinkedHashMap<>()); }
      }
      String code=error instanceof OperationException?((OperationException)error).code:error instanceof SQLException && "40001".equals(((SQLException)error).getSQLState())?"CONFLICT":"DRIVER_ERROR";
      String message=error instanceof OperationException?error.getMessage():"Driver rejected typed operation ["+(error instanceof SQLException?((SQLException)error).getSQLState():"")+ "]; transaction rolled back";
      return terminal=outcome(operation,code.equals("CONFLICT")?"conflict":"failed","0",matched,false,started?"rolled_back":"none",Collections.singletonList(code+": "+message),new LinkedHashMap<>(),new LinkedHashMap<>());
    } finally {
      if(!poisoned) {
        try { if(started)connection.setAutoCommit(true);connection.setTransactionIsolation(previousIsolation);connection.setSchema(previousSchema);
          if(!connection.getAutoCommit() || connection.getTransactionIsolation()!=previousIsolation || !Objects.equals(connection.getSchema(),previousSchema))throw new SQLException(); }
        catch(SQLException cleanupError) { poison(); }
      }
      if(terminal!=null)terminal.put("connection_poisoned",poisoned);
    }
    return outcome(operation,status,affected,matched,true,committed?"committed":"none",poisoned?Collections.singletonList("Committed, but physical connection cleanup failed; reconnect required"):Collections.emptyList(),returned,generated);
  }

  // A strict, bounded JSON parser: duplicate keys/trailing bytes/invalid Unicode
  // and non-JSON numeric tokens are errors; numeric values never pass through double.
  static final class JsonReader {
    final String input;int position;
    JsonReader(String input) { this.input=input; }
    Object parse() throws SQLException { Object value=value(0);space();if(position!=input.length())throw invalid();return value; }
    SQLException invalid() { return fault("INVALID_REQUEST","22023","Malformed JSON payload"); }
    void space() { while(position<input.length() && " \n\r\t".indexOf(input.charAt(position))>=0)position++; }
    Object value(int depth) throws SQLException {
      if(depth>32)throw invalid();space();if(position>=input.length())throw invalid();char c=input.charAt(position);
      if(c=='"')return string();
      if(c=='{') { position++;Map<String,Object> map=new LinkedHashMap<>();space();if(take('}'))return map;
        do { space();if(position>=input.length() || input.charAt(position)!='"')throw invalid();String key=string();space();if(!take(':'))throw invalid();if(map.containsKey(key))throw invalid();map.put(key,value(depth+1));space();if(take('}'))return map; }while(take(','));throw invalid(); }
      if(c=='[') { position++;List<Object> list=new ArrayList<>();space();if(take(']'))return list;
        do { list.add(value(depth+1));if(list.size()>16384)throw invalid();space();if(take(']'))return list; }while(take(','));throw invalid(); }
      for(String word:Arrays.asList("true","false","null"))if(input.startsWith(word,position)) { position+=word.length();return word.equals("null")?null:word.equals("true"); }
      int start=position;if(c=='-')position++;if(position>=input.length())throw invalid();
      if(input.charAt(position)=='0')position++;else { if(input.charAt(position)<'1' || input.charAt(position)>'9')throw invalid();while(position<input.length() && Character.isDigit(input.charAt(position)))position++; }
      if(take('.')) { int before=position;while(position<input.length() && input.charAt(position)>='0' && input.charAt(position)<='9')position++;if(before==position)throw invalid(); }
      if(position<input.length() && (input.charAt(position)=='e' || input.charAt(position)=='E')) { position++;if(position<input.length() && (input.charAt(position)=='+' || input.charAt(position)=='-'))position++;int before=position;while(position<input.length() && input.charAt(position)>='0' && input.charAt(position)<='9')position++;if(before==position)throw invalid(); }
      if(position-start>1024)throw invalid();try { return new java.math.BigDecimal(input.substring(start,position)); }catch(NumberFormatException error) { throw invalid(); }
    }
    boolean take(char c) { if(position<input.length() && input.charAt(position)==c) { position++;return true; }return false; }
    String string() throws SQLException {
      position++;StringBuilder out=new StringBuilder();boolean ended=false;
      while(position<input.length()) { char c=input.charAt(position++);if(c=='"') { ended=true;break; }if(c<32)throw invalid();
        if(c=='\\') { if(position>=input.length())throw invalid();char escaped=input.charAt(position++);switch(escaped) {
          case '"':case '\\':case '/':c=escaped;break;case 'b':c='\b';break;case 'f':c='\f';break;case 'n':c='\n';break;case 'r':c='\r';break;case 't':c='\t';break;
          case 'u':if(position+4>input.length())throw invalid();try { c=(char)Integer.parseInt(input.substring(position,position+4),16); }catch(NumberFormatException error) { throw invalid(); }position+=4;break;
          default:throw invalid();
        } }
        out.append(c);if(out.length()>VALUE_LIMIT)throw invalid();
      }
      if(!ended)throw invalid();for(int i=0;i<out.length();i++) { char c=out.charAt(i);if(Character.isHighSurrogate(c)) { if(++i>=out.length() || !Character.isLowSurrogate(out.charAt(i)))throw invalid(); }else if(Character.isLowSurrogate(c))throw invalid(); }
      return out.toString();
    }
  }
}
final class NavkitJsonString {
  static String quote(String value) {
    StringBuilder out=new StringBuilder("\"");for(int i=0;i<value.length();i++) { char c=value.charAt(i);switch(c) {
      case '"':out.append("\\\"");break;case '\\':out.append("\\\\");break;case '\n':out.append("\\n");break;case '\r':out.append("\\r");break;case '\t':out.append("\\t");break;
      default:if(c<32)out.append(String.format("\\u%04x",(int)c));else out.append(c);
    } }return out.append('"').toString();
  }
}
