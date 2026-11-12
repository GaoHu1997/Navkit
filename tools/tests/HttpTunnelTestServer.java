// Test-only launcher; not copied by build.mjs and not reachable through HTTP.
// Real JDBC calls are counted before invoking H2. Faults occur AFTER real COMMIT.
final class HttpTunnelTestServer extends NavkitHttpTunnel {
  static final AtomicInteger writeCalls=new AtomicInteger(),commits=new AtomicInteger(),closedPhysical=new AtomicInteger(),catalogCalls=new AtomicInteger(),catalogRejections=new AtomicInteger();
  static final Set<Connection> written=Collections.newSetFromMap(new ConcurrentHashMap<Connection,Boolean>());
  static final Set<Connection> faulted=Collections.newSetFromMap(new ConcurrentHashMap<Connection,Boolean>());
  static final Map<Connection,String> aliases=new ConcurrentHashMap<>();
  HttpTunnelTestServer(Map<String,Object> config)throws Exception{super(config);}
  protected Connection sessionConnection(Target target)throws SQLException {
    Connection raw=target.open();AtomicReference<Connection> reference=new AtomicReference<>();AtomicBoolean rolledBack=new AtomicBoolean();
    Connection proxy=(Connection)java.lang.reflect.Proxy.newProxyInstance(getClass().getClassLoader(),new Class[]{Connection.class},(self,method,args)->{
      try {if(method.getName().equals("setAutoCommit")&&Boolean.TRUE.equals(args[0])&&rolledBack.get()&&target.alias.equals("cleanup_read"))throw new SQLException("TEST acknowledged rollback followed by restore failure","08006");Object result=method.invoke(raw,args);if(method.getName().equals("rollback"))rolledBack.set(true);if(method.getName().equals("close")&&raw.isClosed())closedPhysical.incrementAndGet();
        if(method.getName().equals("getMetaData")){
          DatabaseMetaData metadata=(DatabaseMetaData)result;
          return java.lang.reflect.Proxy.newProxyInstance(getClass().getClassLoader(),new Class[]{DatabaseMetaData.class},(ignored,call,values)->{
            try {
              if(call.getName().equals("getTables")){
                catalogCalls.incrementAndGet();
                ResultSet rows=(ResultSet)call.invoke(metadata,values);
                if(target.alias.equals("catalog_denied")){
                  // Consume a real H2 metadata request before injecting the JDBC
                  // rejection. Never substitute a fake HTTP catalog payload.
                  rows.close();catalogRejections.incrementAndGet();
                  throw new SQLException("TEST actual JDBC catalog rejection: catalog-denial-secret-marker","42501");
                }
                return rows;
              }
              return call.invoke(metadata,values);
            }catch(java.lang.reflect.InvocationTargetException error){throw error.getCause();}
          });
        }
        if(method.getName().equals("prepareStatement")){
        PreparedStatement statement=(PreparedStatement)result;return java.lang.reflect.Proxy.newProxyInstance(getClass().getClassLoader(),new Class[]{PreparedStatement.class},(ignored,call,values)->{
          try {if(call.getName().equals("executeLargeUpdate")){writeCalls.incrementAndGet();written.add(reference.get());}return call.invoke(statement,values);}catch(java.lang.reflect.InvocationTargetException error){throw error.getCause();}
        });
      }return result;}catch(java.lang.reflect.InvocationTargetException error){throw error.getCause();}
    });reference.set(proxy);aliases.put(proxy,target.alias);return proxy;
  }
  protected void commit(Connection connection)throws SQLException {
    if("slow_commit".equals(aliases.get(connection))&&written.contains(connection)){
      System.out.println(encode(map("commit_waiting",true)));System.out.flush();try{Thread.sleep(250);}catch(InterruptedException error){Thread.currentThread().interrupt();throw new SQLException("interrupted","08006");}
    }
    connection.commit();commits.incrementAndGet();
    if("ack_lost".equals(aliases.get(connection))&&written.contains(connection)&&faulted.add(connection))throw new SQLException("TEST real commit completed; acknowledgment lost","08006");
  }
  public static void main(String[] args)throws Exception {
    Map<String,Object> config=object(new JdbcMutationSupport.JsonReader(Files.readString(Paths.get(args[0]),StandardCharsets.UTF_8)).parse());
    try(HttpTunnelTestServer service=new HttpTunnelTestServer(config)) {
      service.start();System.out.println(encode(map("ready",true,"endpoint",service.endpoint(),"server_id",service.serverId,"server_generation",service.serverGeneration,"protocol",PROTOCOL)));System.out.flush();
      BufferedReader input=new BufferedReader(new InputStreamReader(System.in,StandardCharsets.UTF_8));for(String command;(command=input.readLine())!=null;){
        if(command.equals("inspect")){System.out.println(encode(map("inspection",true,"write_calls",writeCalls.get(),"commits",commits.get(),"closed_physical",closedPhysical.get(),"catalog_calls",catalogCalls.get(),"catalog_rejections",catalogRejections.get(),"sessions",service.sessions.size(),"status_records",service.executions.size())));System.out.flush();}
        else if(command.equals("stop"))break;
      }
    }
  }
}
