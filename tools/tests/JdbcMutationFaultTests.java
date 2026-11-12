import java.sql.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

// Real H2 database with JDBC boundary fault injection, not vendor certification.
class JdbcMutationFaultTests {
  static int checks;
  static void check(boolean condition,String message) {
    if(!condition)throw new AssertionError(message);checks++;
  }
  static Map<String,Object> val(String kind,Object value) { return JdbcMutationSupport.dbValue(kind,value); }
  static List<Object> pair(String name,Map<String,Object> value) { return Arrays.asList(name,value); }
  static List<Object> pairs(Object... items) { return Arrays.asList(items); }
  static String scalar(Connection connection,String sql) throws SQLException {
    try(Statement query=connection.createStatement();ResultSet rows=query.executeQuery(sql)) { check(rows.next(),"scalar row missing");return rows.getString(1); }
  }
  static Object call(java.lang.reflect.Method method,Object target,Object[] args) throws Throwable {
    try { return method.invoke(target,args); }catch(java.lang.reflect.InvocationTargetException error) { throw error.getCause(); }
  }
  static final class Fixture implements AutoCloseable {
    final Connection raw,observer,wrapped;
    final JdbcMutationSupport support;
    final Map<String,Object> id;
    String mode="normal";
    int executes, commits, rollbacks, decimalBinds, doubleBinds, nullBinds, ownedStatements;
    boolean injected, committed;
    final List<String> prepared=new ArrayList<>();
    Fixture() throws Exception {
      String url="jdbc:h2:mem:pl20_fault_"+UUID.randomUUID().toString().replace("-","")+";DB_CLOSE_DELAY=-1";
      raw=DriverManager.getConnection(url,"sa","");observer=DriverManager.getConnection(url,"sa","");
      try(Statement query=raw.createStatement()) {
        query.execute("CREATE TABLE ITEM(ID BIGINT PRIMARY KEY,PAYLOAD VARCHAR(100),D DECIMAL(38,12))");
        query.execute("INSERT INTO ITEM VALUES(1,'old',1.234567890123)");
      }
      wrapped=(Connection)java.lang.reflect.Proxy.newProxyInstance(Fixture.class.getClassLoader(),new Class<?>[]{Connection.class},(proxy,method,args)-> {
        String name=method.getName();
        if(name.equals("commit")) { commits++;Object result=call(method,raw,args);committed=true;
          if(mode.equals("commit-ack"))throw new SQLException("Commit transport acknowledgement lost with confidential data","08006");return result; }
        if(name.equals("rollback")) { rollbacks++;Object result=call(method,raw,args);
          if(mode.equals("rollback-ack"))throw new SQLException("Rollback transport acknowledgement lost","08006");return result; }
        if(name.equals("setAutoCommit") && Boolean.TRUE.equals(args[0]) && committed && mode.equals("cleanup"))throw new SQLException("Cleanup failed","08006");
        if(mode.equals("warehouse-features") && Arrays.asList("isReadOnly","getSchema","getCatalog").contains(name))throw new SQLFeatureNotSupportedException("Read-only warehouse API is unavailable");
        if(mode.equals("warehouse-abstract") && Arrays.asList("isReadOnly","getSchema","getCatalog").contains(name))throw new AbstractMethodError("Legacy JDBC write/namespace API is missing");
        if(name.equals("unwrap") && mode.equals("object-generation-unavailable"))throw new SQLFeatureNotSupportedException("Actual schema object generation is unavailable");
        if(name.equals("getMetaData") && (mode.equals("unknown-metadata") || mode.equals("uncertified-product") || mode.equals("warehouse-features") || mode.equals("warehouse-abstract") || mode.equals("metadata-error-null"))) {
          DatabaseMetaData metadata=raw.getMetaData();
          return java.lang.reflect.Proxy.newProxyInstance(Fixture.class.getClassLoader(),new Class<?>[]{DatabaseMetaData.class},(p,m,a)-> {
            if(mode.equals("unknown-metadata") && m.getName().equals("getPrimaryKeys"))throw new SQLFeatureNotSupportedException("Keys not available");
            if(mode.equals("metadata-error-null") && m.getName().equals("getPrimaryKeys"))throw new SQLException("Secret metadata failure");
            if((mode.equals("uncertified-product") || mode.equals("warehouse-features") || mode.equals("warehouse-abstract")) && m.getName().equals("getDatabaseProductName"))return "Uncertified Cloud JDBC";
            if(mode.equals("warehouse-features") && Arrays.asList("getDriverName","getDriverVersion","supportsTransactions","supportsTransactionIsolationLevel").contains(m.getName()))throw new SQLFeatureNotSupportedException("Warehouse transaction/driver API unavailable");
            if(mode.equals("warehouse-abstract") && Arrays.asList("getDriverName","getDriverVersion","supportsTransactions","supportsTransactionIsolationLevel").contains(m.getName()))throw new AbstractMethodError("Legacy JDBC metadata API is missing");
            return call(m,metadata,a);
          });
        }
        Object result=call(method,raw,args);
        if(name.equals("prepareStatement")) {
          String sql=(String)args[0];prepared.add(sql);ownedStatements++;
          PreparedStatement statement=(PreparedStatement)result;
          return java.lang.reflect.Proxy.newProxyInstance(Fixture.class.getClassLoader(),new Class<?>[]{PreparedStatement.class},(p,m,a)-> {
            String operation=m.getName();
            if(operation.equals("setBigDecimal"))decimalBinds++;
            if(operation.equals("setDouble"))doubleBinds++;
            if(operation.equals("setNull"))nullBinds++;
            if(operation.equals("executeQuery") && sql.endsWith(" FOR UPDATE") && !injected) {
              injected=true;
              if(mode.equals("metadata-race"))try(Statement external=observer.createStatement()) { external.execute("ALTER TABLE ITEM ADD EXTRA INTEGER DEFAULT 0"); }
              if(mode.equals("row-race"))try(Statement external=observer.createStatement()) { external.execute("UPDATE ITEM SET PAYLOAD='external' WHERE ID=1"); }
            }
            Object response=call(m,statement,a);
            if(operation.equals("executeLargeUpdate")) {
              executes++;
              if(mode.equals("count-zero"))return 0L;
              if(mode.equals("count-many"))return 2L;
              if(mode.equals("count-unknown"))return (long)Statement.SUCCESS_NO_INFO;
              if(mode.equals("execute-throws"))throw new SQLFeatureNotSupportedException("Driver threw after sending DML");
            }
            if(operation.equals("executeQuery") && sql.endsWith(" FOR UPDATE") && mode.equals("match-many")) {
              ResultSet rows=(ResultSet)response;int[] sequence={0};
              return java.lang.reflect.Proxy.newProxyInstance(Fixture.class.getClassLoader(),new Class<?>[]{ResultSet.class},(r,rm,ra)-> {
                if(rm.getName().equals("next")) { sequence[0]++;if(sequence[0]==1)return rows.next();if(sequence[0]==2)return true;return false; }
                return call(rm,rows,ra);
              });
            }
            if(operation.equals("getGeneratedKeys") && mode.equals("generated-mismatch")) {
              ResultSet rows=(ResultSet)response;
              return java.lang.reflect.Proxy.newProxyInstance(Fixture.class.getClassLoader(),new Class<?>[]{ResultSet.class},(r,rm,ra)-> {
                Object value=call(rm,rows,ra);
                if(rm.getName().equals("getBigDecimal") && value!=null)return ((java.math.BigDecimal)value).add(new java.math.BigDecimal("99"));
                return value;
              });
            }
            return response;
          });
        }
        return result;
      });
      support=new JdbcMutationSupport(wrapped,"fault-fixture");
      id=JdbcMutationSupport.map("catalog",raw.getCatalog(),"schema","PUBLIC","name","ITEM");
    }
    Map<String,Object> metadata() throws SQLException { return support.metadataScoped(id); }
    Map<String,Object> plan(Map<String,Object> metadata,String kind,List<Object> changes,List<Object> keys,List<Object> original) {
      return JdbcMutationSupport.map("operation_id",UUID.randomUUID().toString(),"session_id",support.sessionId,"object",id,
        "base_revision",metadata.get("revision"),"kind",kind,"changes",changes,"keys",keys,"original_values",original,"read_only",false,"timeout_secs",new java.math.BigDecimal("10"));
    }
    Map<String,Object> update(String value) throws SQLException {
      Map<String,Object> metadata=metadata();
      return support.mutate(plan(metadata,"update",pairs(pair("PAYLOAD",val("text",value))),pairs(pair("ID",val("integer","1"))),pairs(pair("PAYLOAD",val("text","old")))));
    }
    String payload() throws SQLException { return scalar(observer,"SELECT PAYLOAD FROM ITEM WHERE ID=1"); }
    @Override public void close() throws SQLException { if(!raw.isClosed())raw.close();observer.close(); }
  }

  static void counts() throws Exception {
    for(String mode:Arrays.asList("count-zero","count-many","count-unknown","execute-throws"))try(Fixture fixture=new Fixture()) {
      fixture.mode=mode;Map<String,Object> result=fixture.update("requested");
      check(!Boolean.TRUE.equals(result.get("verified")),mode+" cannot verify success");
      check("rolled_back".equals(result.get("transaction")),mode+" must rollback");
      check("0".equals(result.get("affected_rows")),mode+" reports no committed rows");
      check("old".equals(fixture.payload()),mode+" database must remain unchanged");
      check(fixture.executes==1 && fixture.commits==0 && fixture.rollbacks==1,mode+" must not retry any execute API");
      System.out.println("PASS count/refusal/no-replay "+mode);
    }
    try(Fixture fixture=new Fixture()) {
      fixture.mode="count-zero";Map<String,Object> result=fixture.update("old");
      check("unchanged".equals(result.get("status")) && Boolean.TRUE.equals(result.get("verified")),"locked exact no-op succeeds");
      check(fixture.executes==0 && fixture.commits==1,"no-op must not rely on arbitrary raw-zero driver results");
      System.out.println("PASS locked no-op executes no DML");
    }
    try(Fixture fixture=new Fixture()) {
      fixture.mode="match-many";Map<String,Object> result=fixture.update("requested");
      check("2".equals(result.get("matched_rows")) && "conflict".equals(result.get("status")),"multi-row CAS must conflict");
      check(fixture.executes==0 && "old".equals(fixture.payload()),"multi-row CAS must refuse before DML");
      System.out.println("PASS multi-row match refuses before write");
    }
    try(Fixture fixture=new Fixture()) {
      Map<String,Object> result=fixture.support.mutate(fixture.plan(fixture.metadata(),"update",Collections.emptyList(),pairs(pair("ID",val("integer","1"))),pairs(pair("PAYLOAD",val("text","old")))));
      check("failed".equals(result.get("status")) && Boolean.FALSE.equals(result.get("verified")),"empty update cannot fabricate a verified no-op");
      check(fixture.executes==0 && fixture.commits==0 && "0".equals(result.get("affected_rows")),"empty update sends no DML and commits nothing");
      check("old".equals(fixture.payload()),"empty update preserves the actual old row");
      System.out.println("PASS empty update is not a fabricated verified no-op");
    }
  }

  static void racesAndTriggers() throws Exception {
    try(Fixture fixture=new Fixture()) {
      fixture.mode="metadata-race";Map<String,Object> result=fixture.update("requested");
      check("conflict".equals(result.get("status")) && fixture.executes==0,"real external ALTER between snapshot and lock must refuse");
      check("old".equals(fixture.payload()),"metadata race does not modify row");
      System.out.println("PASS real concurrent metadata DDL before row lock");
    }
    try(Fixture fixture=new Fixture()) {
      fixture.mode="row-race";Map<String,Object> result=fixture.update("requested");
      check("conflict".equals(result.get("status")) && ("0".equals(result.get("matched_rows"))
        || result.get("matched_rows")==null && ((List<?>)result.get("mismatches")).toString().contains("40001")),
        "real external row change must CAS-conflict; SERIALIZABLE abort keeps an unknown match count: "+JdbcMutationSupport.encode(result));
      check("external".equals(fixture.payload()) && fixture.executes==0,"own rollback preserves independent external write");
      System.out.println("PASS real concurrent row CAS conflict");
    }
    try(Fixture fixture=new Fixture()) {
      String source="org.h2.api.Trigger create() { return new org.h2.api.Trigger() { public void fire(java.sql.Connection c, Object[] oldRow, Object[] newRow) { newRow[1] = \"trigger-rewrite\"; } }; }";
      try(Statement query=fixture.raw.createStatement()) { query.execute("CREATE TRIGGER REWRITE BEFORE UPDATE ON ITEM FOR EACH ROW AS '"+source.replace("'","''")+"'"); }
      Map<String,Object> metadata=fixture.metadata();check(!((List<?>)metadata.get("triggers")).isEmpty(),"real trigger definition in revision");
      Map<String,Object> result=fixture.support.mutate(fixture.plan(metadata,"update",pairs(pair("PAYLOAD",val("text","requested"))),pairs(pair("ID",val("integer","1"))),Collections.emptyList()));
      check("conflict".equals(result.get("status")) && "rolled_back".equals(result.get("transaction")),"real trigger target mismatch must rollback");
      check("old".equals(fixture.payload()),"trigger-modified wrong value never committed");
      System.out.println("PASS real trigger rewrite fails typed readback and rolls back");
    }
  }

  static void acknowledgements() throws Exception {
    try(Fixture fixture=new Fixture()) {
      fixture.mode="commit-ack";Map<String,Object> result=fixture.update("possibly-committed");
      check("unknown".equals(result.get("status")) && "unknown".equals(result.get("transaction")),"missing commit ACK is unknown");
      check(result.get("affected_rows")==null && Boolean.FALSE.equals(result.get("verified")),"unknown commit never fabricates count/success");
      check(Boolean.TRUE.equals(result.get("connection_poisoned")) && fixture.raw.isClosed(),"unknown commit destroys owned physical connection");
      check("possibly-committed".equals(fixture.payload()),"real committed database proves why automatic retry is dangerous");
      check(fixture.executes==1 && fixture.commits==1 && fixture.rollbacks==0,"unknown commit does not retry/pretend rollback");
      try { fixture.support.mutate(JdbcMutationSupport.map());throw new AssertionError("invalid poisoned plan should fail"); }catch(SQLException expected) { }
      check(fixture.executes==1,"poisoned sidecar does not replay");
      System.out.println("PASS lost commit acknowledgement: real commit, UNKNOWN, close, no replay");
    }
    try(Fixture fixture=new Fixture()) {
      fixture.mode="rollback-ack";
      Map<String,Object> metadata=fixture.metadata();Map<String,Object> result=fixture.support.mutate(fixture.plan(metadata,"update",
        pairs(pair("PAYLOAD",val("text","requested"))),pairs(pair("ID",val("integer","1"))),pairs(pair("PAYLOAD",val("text","stale")))));
      check("unknown".equals(result.get("status")) && Boolean.TRUE.equals(result.get("connection_poisoned")),"missing rollback ACK is unknown and discarded");
      check(fixture.raw.isClosed() && fixture.rollbacks==1 && fixture.commits==0,"unknown rollback never returns connection to pool");
      check("old".equals(fixture.payload()),"actual rollback observed only by independent connection");
      System.out.println("PASS missing rollback acknowledgement closes session, no replay");
    }
    try(Fixture fixture=new Fixture()) {
      fixture.mode="cleanup";Map<String,Object> result=fixture.update("committed");
      check(Boolean.TRUE.equals(result.get("verified")) && "committed".equals(result.get("transaction")),"acknowledged commit remains acknowledged if cleanup fails");
      check(Boolean.TRUE.equals(result.get("connection_poisoned")) && fixture.raw.isClosed(),"failed connection cleanup cannot be returned for reuse");
      check("committed".equals(fixture.payload()),"independent readback proves actual commit");
      System.out.println("PASS postcommit cleanup closes connection without falsifying commit");
    }
  }

  static void generatedKeysAndBindings() throws Exception {
    try(Fixture fixture=new Fixture()) {
      String secret="bound_'not SQL;中文";
      Map<String,Object> metadata=fixture.metadata();Map<String,Object> result=fixture.support.mutate(fixture.plan(metadata,"update",
        pairs(pair("PAYLOAD",val("text",secret)),pair("D",val("decimal","12345678901234567890.123456789012"))),pairs(pair("ID",val("integer","1"))),Collections.emptyList()));
      check(Boolean.TRUE.equals(result.get("verified")),"typed private text+exact decimal actually commit");
      check(fixture.prepared.stream().noneMatch(sql->sql.contains(secret)),"row values never enter SQL templates");
      check(fixture.decimalBinds>0 && fixture.doubleBinds==0,"DECIMAL binds use BigDecimal, not double");
      check(fixture.ownedStatements>0 && fixture.executes==1 && fixture.commits==1,"same wrapped physical connection owns every operation");
      System.out.println("PASS PreparedStatement/BigDecimal/owned-physical-connection contract");
    }
    try(Fixture fixture=new Fixture()) {
      try(Statement query=fixture.raw.createStatement()) { query.execute("CREATE TABLE GENERATED(ID BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,PAYLOAD VARCHAR(100),D DECIMAL(38,12))"); }
      Map<String,Object> id=JdbcMutationSupport.map("catalog",fixture.raw.getCatalog(),"schema","PUBLIC","name","GENERATED");
      Map<String,Object> metadata=fixture.support.metadataScoped(id);
      Map<String,Object> plan=fixture.plan(metadata,"insert",pairs(pair("PAYLOAD",val("null",null)),pair("D",val("null",null))),Collections.emptyList(),Collections.emptyList());plan.put("object",id);
      Map<String,Object> result=fixture.support.mutate(plan);
      check(Boolean.TRUE.equals(result.get("verified")) && fixture.nullBinds==2,"NULLs use typed setNull and generated identity is read back");
      check(((List<?>)result.get("generated_keys")).size()==1,"generated keys returned from same owned PreparedStatement");
      check("1".equals(scalar(fixture.observer,"SELECT COUNT(*) FROM GENERATED WHERE PAYLOAD IS NULL AND D IS NULL")),"NULL is not empty string");
      fixture.mode="generated-mismatch";metadata=fixture.support.metadataScoped(id);
      plan=fixture.plan(metadata,"insert",pairs(pair("PAYLOAD",val("text","cannot-commit"))),Collections.emptyList(),Collections.emptyList());plan.put("object",id);
      result=fixture.support.mutate(plan);
      check(Boolean.FALSE.equals(result.get("verified")) && "rolled_back".equals(result.get("transaction")),"incorrect generated-key readback never commits");
      check("1".equals(scalar(fixture.observer,"SELECT COUNT(*) FROM GENERATED")),"bad generated-key insert rolled back");
      System.out.println("PASS typed NULL, same-transaction generated keys and incorrect-key rollback");
    }
    try(Fixture fixture=new Fixture()) {
      fixture.mode="unknown-metadata";
      try { fixture.metadata();throw new AssertionError("missing key metadata cannot be empty success"); }catch(SQLFeatureNotSupportedException expected) { }
      check(fixture.executes==0,"unknown keys do not write");
      fixture.mode="uncertified-product";
      check(Boolean.FALSE.equals(fixture.support.capabilities().get("row_write")),"unverified JDBC product cannot be enabled by shared trait");
      System.out.println("PASS incomplete metadata and uncertified JDBC product stay readonly");
    }
    try(Fixture fixture=new Fixture()) {
      fixture.mode="warehouse-features";Map<String,Object> caps=fixture.support.capabilities();
      check(Boolean.FALSE.equals(caps.get("row_write")) && Boolean.FALSE.equals(caps.get("metadata")),"warehouse lacks certified metadata/typed mutations");
      check(Boolean.TRUE.equals(caps.get("read_only")) && Boolean.FALSE.equals(caps.get("transactions")),"missing warehouse write APIs are conservatively disabled, not called to break browsing");
      check("1".equals(scalar(fixture.raw,"SELECT 1")),"ordinary legacy read remains available");
      fixture.mode="metadata-error-null";
      StringWriter output=new StringWriter();PrintWriter writer=new PrintWriter(output,true);
      Map<String,Object> request=JdbcMutationSupport.map("protocol",new java.math.BigDecimal("2"),"request_id",UUID.randomUUID().toString(),"session_id",fixture.support.sessionId,
        "action","metadata","payload",JdbcMutationSupport.map("object",fixture.id));
      fixture.support.handle(Base64.getEncoder().encodeToString(JdbcMutationSupport.encode(request).getBytes(StandardCharsets.UTF_8)),writer,"secret");
      Map<String,Object> envelope=JdbcMutationSupport.object(new JdbcMutationSupport.JsonReader(output.toString().trim()).parse());
      Map<String,Object> error=JdbcMutationSupport.object(envelope.get("error"));
      check("".equals(error.get("sql_state")) && "none".equals(error.get("effects")),"null SQLState retains explicit string/error/no-write evidence");
      check(!output.toString().contains("Secret metadata failure") && !fixture.raw.isClosed(),"driver details do not leak and an ordinary metadata refusal need not discard a clean read session");
      System.out.println("PASS unsupported warehouse APIs preserve readonly browsing and explicit error metadata");
    }
    try(Fixture fixture=new Fixture()) {
      fixture.mode="warehouse-abstract";Map<String,Object> caps=fixture.support.capabilities();
      check(Boolean.FALSE.equals(caps.get("row_write")) && Boolean.TRUE.equals(caps.get("read_only")),"missing legacy JDBC methods cannot imply writable capability");
      check("".equals(caps.get("catalog")) && "".equals(caps.get("schema")),"missing namespace methods are explicit uncertified metadata, not fabricated names");
      check("1".equals(scalar(fixture.raw,"SELECT 1")),"capability discovery does not break an existing ordinary read");
      System.out.println("PASS legacy absent JDBC methods stay uncertified without breaking reads");
    }
  }

  static void objectRecreationAba() throws Exception {
    try(Fixture fixture=new Fixture()) {
      Map<String,Object> before=fixture.metadata();
      Map<String,Object> plan=fixture.plan(before,"update",pairs(pair("PAYLOAD",val("text","must-not-write"))),
        pairs(pair("ID",val("integer","1"))),pairs(pair("PAYLOAD",val("text","old"))));
      try(Statement observer=fixture.observer.createStatement()) {
        observer.execute("DROP TABLE ITEM");
        observer.execute("CREATE TABLE ITEM(ID BIGINT PRIMARY KEY,PAYLOAD VARCHAR(100),D DECIMAL(38,12))");
        observer.execute("INSERT INTO ITEM VALUES(1,'old',1.234567890123)");
      }
      Map<String,Object> after=fixture.metadata();
      check(before.get("backend_object_id").equals(after.get("backend_object_id")),"real H2 reuses numeric object ID during ABA");
      check(before.get("definition").equals(after.get("definition")),"same recreated DDL cannot distinguish actual object instance");
      check(!before.get("revision").equals(after.get("revision")),"actual Table generation changes despite identical numeric ID/DDL/row");
      Map<String,Object> result=fixture.support.mutate(plan);
      check("conflict".equals(result.get("status")) && Boolean.FALSE.equals(result.get("verified")),"old object generation plan cannot mutate replacement row");
      check(fixture.executes==0 && "0".equals(result.get("affected_rows")),"ABA fails before sending any DML");
      check("old".equals(scalar(fixture.observer,"SELECT PAYLOAD FROM ITEM WHERE ID=1")),"identical replacement row remains unmodified");
      check(!fixture.raw.isClosed(),"known ABA metadata conflict keeps a clean rolled-back session");
      System.out.println("PASS real numeric object ID/DDL ABA recreation is rejected using actual physical Table generation");
    }
    try(Fixture fixture=new Fixture()) {
      fixture.mode="object-generation-unavailable";
      Map<String,Object> caps=fixture.support.capabilities();
      check(Boolean.FALSE.equals(caps.get("row_write")) && Boolean.FALSE.equals(caps.get("metadata")),"unavailable actual object generation cannot certify writes");
      check("1".equals(scalar(fixture.raw,"SELECT 1")),"uncertified generation keeps ordinary legacy browsing available");
      check(fixture.executes==0,"uncertified transport/session does not send DML");
      System.out.println("PASS unavailable/uncertified schema-object generation stays read-only without breaking browsing");
    }
  }

  static void codec() throws Exception {
    Object exact=new JdbcMutationSupport.JsonReader("{\"v\":9223372036854775807,\"d\":12345678901234567890.123456789012,\"s\":\"中文\\t尾🙂\"}").parse();
    Map<String,Object> map=JdbcMutationSupport.object(exact);check(new java.math.BigDecimal("9223372036854775807").equals(map.get("v")),"JSON integer exact, not double");
    check(new java.math.BigDecimal("12345678901234567890.123456789012").equals(map.get("d")),"JSON decimal exact, not double");
    for(String malformed:Arrays.asList("{\"x\":1,\"x\":2}","[1,]","{} trailing","01","NaN","{\"x\":\"\\uD800\"}","{\"x\":\"\\uDC00\"}")) {
      try { new JdbcMutationSupport.JsonReader(malformed).parse();throw new AssertionError("malformed JSON was accepted: "+malformed); }catch(SQLException expected) { checks++; }
    }
    try { JdbcMutationSupport.readLine(new BufferedReader(new StringReader("partial")));throw new AssertionError("partial frame accepted"); }catch(EOFException expected) { checks++; }
    check("one".equals(JdbcMutationSupport.readLine(new BufferedReader(new StringReader("one\n")))),"complete bounded frame accepted");
    System.out.println("PASS exact JSON, malformed/duplicate/trailing/Unicode/partial-frame rejection");
  }
  public static void main(String[] args) throws Exception {
    Class.forName("org.h2.Driver");counts();racesAndTriggers();acknowledgements();generatedKeysAndBindings();objectRecreationAba();codec();
    System.out.println("PL20_FAULT_TESTS_PASS "+checks);
  }
}
